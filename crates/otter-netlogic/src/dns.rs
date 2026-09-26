//! Sans-I/O DNS A-record lookup with CNAME support.
//! Works on UDP payloads (DNS messages), never builds frames.

use alloc::vec::Vec;
use alloc::boxed::Box;
use core::cmp::min;

// ============================================================================
// Public API
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    InvalidName,
    NotFound,
    ServerFailure,
    Timeout,
    CnameLoop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Wait { until_ms: u64 },
    Send(Vec<u8>),
    Done(Result<[u8; 4], LookupError>),
}

pub struct Lookup {
    // Configuration
    #[allow(dead_code)]
    name: NameBuf,               // original query name
    #[allow(dead_code)]
    now_start_ms: u64,           // when the lookup started
    deadline_ms: u64,            // start + timeout_ms
    ids: Box<dyn FnMut() -> u16>,

    // Current state
    current_name: NameBuf,       // name we're currently resolving
    current_id: u16,             // id of current query
    query_sent_at: u64,          // when current query was sent
    next_retransmit_gap_ms: u64, // 1, 2, 4, 8, ... seconds
    cname_hops: u32,             // 0..8 hops taken so far

    // Result (when Done)
    result: Option<Result<[u8; 4], LookupError>>,
}

impl Lookup {
    /// Start a DNS lookup for an A record.
    /// Returns (Lookup, first_query_bytes) or InvalidName.
    pub fn start(
        name: &str,
        now_ms: u64,
        timeout_ms: u64,
        mut ids: Box<dyn FnMut() -> u16>,
    ) -> Result<(Lookup, Vec<u8>), LookupError> {
        let name = NameBuf::parse(name)?;
        let current_id = ids();
        let query = build_query(&name, current_id)?;

        Ok((
            Lookup {
                name: name.clone(),
                now_start_ms: now_ms,
                deadline_ms: now_ms + timeout_ms,
                ids,
                current_name: name,
                current_id,
                query_sent_at: now_ms,
                next_retransmit_gap_ms: 1000, // first gap is 1 second
                cname_hops: 0,
                result: None,
            },
            query,
        ))
    }

    /// Process a response from the server.
    pub fn on_response(&mut self, msg: &[u8], now_ms: u64) -> Step {
        if let Some(r) = &self.result {
            return Step::Done(r.clone());
        }

        // Parse and validate the response
        match parse_response(msg, &self.current_name, self.current_id, now_ms, self.cname_hops) {
            Ok((ResponseAction::A(ip), _hops)) => {
                self.result = Some(Ok(ip));
                Step::Done(Ok(ip))
            }
            Ok((ResponseAction::CnameTarget(target), hops)) => {
                // Follow the CNAME
                self.cname_hops = hops;
                if self.cname_hops > 8 {
                    self.result = Some(Err(LookupError::CnameLoop));
                    return Step::Done(Err(LookupError::CnameLoop));
                }

                self.current_name = target;
                self.current_id = (self.ids)();
                self.query_sent_at = now_ms;
                self.next_retransmit_gap_ms = 1000;

                match build_query(&self.current_name, self.current_id) {
                    Ok(q) => Step::Send(q),
                    Err(e) => {
                        self.result = Some(Err(e.clone()));
                        Step::Done(Err(e))
                    }
                }
            }
            Ok((ResponseAction::NotFound, _hops)) => {
                self.result = Some(Err(LookupError::NotFound));
                Step::Done(Err(LookupError::NotFound))
            }
            Ok((ResponseAction::ServerFailure, _hops)) => {
                self.result = Some(Err(LookupError::ServerFailure));
                Step::Done(Err(LookupError::ServerFailure))
            }
            Ok((ResponseAction::CnameLoop, _hops)) => {
                self.result = Some(Err(LookupError::CnameLoop));
                Step::Done(Err(LookupError::CnameLoop))
            }
            Err(_) => {
                // Invalid message, wait for the next one
                Step::Wait { until_ms: self.next_wait_time(now_ms) }
            }
        }
    }

    /// Poll the lookup for timeouts and retransmissions.
    pub fn poll(&mut self, now_ms: u64) -> Step {
        if let Some(r) = &self.result {
            return Step::Done(r.clone());
        }

        // Check for overall timeout
        if now_ms >= self.deadline_ms {
            self.result = Some(Err(LookupError::Timeout));
            return Step::Done(Err(LookupError::Timeout));
        }

        // Check if it's time to retransmit
        let retransmit_time = self.query_sent_at + self.next_retransmit_gap_ms;

        if now_ms >= retransmit_time {
            // Time to retransmit
            let query = match build_query(&self.current_name, self.current_id) {
                Ok(q) => q,
                Err(e) => {
                    self.result = Some(Err(e.clone()));
                    return Step::Done(Err(e));
                }
            };

            self.query_sent_at = now_ms;
            self.next_retransmit_gap_ms *= 2;

            Step::Send(query)
        } else {
            // Wait until the next retransmission or deadline, whichever comes first
            Step::Wait { until_ms: self.next_wait_time(now_ms) }
        }
    }

    fn next_wait_time(&self, _now_ms: u64) -> u64 {
        let next_retransmit = self.query_sent_at + self.next_retransmit_gap_ms;
        min(next_retransmit, self.deadline_ms)
    }
}

// ============================================================================
// Internal: Name parsing and encoding
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
struct NameBuf {
    labels: Vec<Vec<u8>>,
}

impl NameBuf {
    /// Parse and validate a domain name.
    /// Format: labels separated by '.', optional trailing '.', labels are [A-Za-z0-9_-] only.
    fn parse(name: &str) -> Result<NameBuf, LookupError> {
        // Remove at most one trailing dot
        let name = name.strip_suffix('.').unwrap_or(name);

        if name.is_empty() {
            return Err(LookupError::InvalidName);
        }

        if name.len() > 253 {
            return Err(LookupError::InvalidName);
        }

        // Check for ".." anywhere
        if name.contains("..") {
            return Err(LookupError::InvalidName);
        }

        // Check if starts or ends with '.'
        if name.starts_with('.') || name.ends_with('.') {
            return Err(LookupError::InvalidName);
        }

        let mut labels = Vec::new();
        for label in name.split('.') {
            if label.is_empty() || label.len() > 63 {
                return Err(LookupError::InvalidName);
            }

            // Check label characters: [A-Za-z0-9_-] only
            for &b in label.as_bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' => {}
                    _ => return Err(LookupError::InvalidName),
                }
            }

            labels.push(label.as_bytes().to_vec());
        }

        if labels.is_empty() {
            return Err(LookupError::InvalidName);
        }

        Ok(NameBuf { labels })
    }

    /// Encode name as DNS format: length-prefixed labels + 0 terminator.
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for label in &self.labels {
            out.push(label.len() as u8);
            out.extend_from_slice(label);
        }
        out.push(0);
        out
    }
}

// ============================================================================
// Internal: Query building
// ============================================================================

fn build_query(name: &NameBuf, id: u16) -> Result<Vec<u8>, LookupError> {
    let mut q = Vec::new();

    // Header
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD
    q.extend_from_slice(&1u16.to_be_bytes());     // QDCOUNT
    q.extend_from_slice(&0u16.to_be_bytes());     // ANCOUNT
    q.extend_from_slice(&0u16.to_be_bytes());     // NSCOUNT
    q.extend_from_slice(&0u16.to_be_bytes());     // ARCOUNT

    // Question
    q.extend_from_slice(&name.encode());
    q.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    q.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN

    Ok(q)
}

// ============================================================================
// Internal: Response parsing
// ============================================================================

enum ResponseAction {
    A([u8; 4]),
    CnameTarget(NameBuf),
    NotFound,
    ServerFailure,
    CnameLoop,
}

fn parse_response(
    msg: &[u8],
    expected_name: &NameBuf,
    expected_id: u16,
    _now_ms: u64,
    initial_hops: u32,
) -> Result<(ResponseAction, u32), LookupError> {
    // Minimum header size
    if msg.len() < 12 {
        return Err(LookupError::InvalidName);
    }

    // Check ID
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    if id != expected_id {
        return Err(LookupError::InvalidName);
    }

    // Check QR, opcode, question match
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    if (flags & 0x8000) == 0 {
        return Err(LookupError::InvalidName); // QR not set
    }

    if (flags & 0x7800) != 0 {
        return Err(LookupError::InvalidName); // opcode != 0
    }

    let rcode = (flags & 0x000f) as u8;
    let tc = (flags & 0x0200) != 0;

    // Check question section
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let ancount = u16::from_be_bytes([msg[6], msg[7]]) as usize;

    if qdcount != 1 {
        return Err(LookupError::InvalidName);
    }

    // Parse and validate the question
    let mut offset = 12usize;
    let (new_offset, q_name) = parse_name_at(msg, offset, &mut LoopDetector::new())?;
    offset = new_offset;

    if offset + 4 > msg.len() {
        return Err(LookupError::InvalidName);
    }

    let qtype = u16::from_be_bytes([msg[offset], msg[offset + 1]]);
    let qclass = u16::from_be_bytes([msg[offset + 2], msg[offset + 3]]);
    offset += 4;

    // Check question matches (case-insensitive)
    if !names_equal(expected_name, &q_name) {
        return Err(LookupError::InvalidName);
    }

    if qtype != 1 || qclass != 1 {
        return Err(LookupError::InvalidName);
    }

    // Handle error responses
    if tc || (rcode != 0 && rcode != 3) {
        return Ok((ResponseAction::ServerFailure, initial_hops));
    }

    if rcode == 3 {
        return Ok((ResponseAction::NotFound, initial_hops));
    }

    // Process CNAME chain in the answer section
    let mut current = expected_name.clone();
    let mut visited = Vec::new();
    visited.push(current.clone());
    let mut hops = initial_hops;

    // Parse answer section (rcode == 0)
    let answer_offset = offset;
    loop {
        let mut found_record = false;

        // Scan the answer section for a record owned by current
        offset = answer_offset;
        for _ in 0..ancount {
            if offset >= msg.len() {
                return Err(LookupError::InvalidName);
            }

            let (new_offset, rr_name) = parse_name_at(msg, offset, &mut LoopDetector::new())?;
            offset = new_offset;

            if offset + 10 > msg.len() {
                return Err(LookupError::InvalidName);
            }

            let rtype = u16::from_be_bytes([msg[offset], msg[offset + 1]]);
            let rclass = u16::from_be_bytes([msg[offset + 2], msg[offset + 3]]);
            let _ttl = u32::from_be_bytes([msg[offset + 4], msg[offset + 5], msg[offset + 6], msg[offset + 7]]);
            let rdlen = u16::from_be_bytes([msg[offset + 8], msg[offset + 9]]) as usize;
            offset += 10;

            if offset + rdlen > msg.len() {
                return Err(LookupError::InvalidName);
            }

            let rdata = &msg[offset..offset + rdlen];

            // Only consider records owned by the current name and class IN
            if names_equal(&rr_name, &current) && rclass == 1 {
                match rtype {
                    1 => {
                        // A record
                        if rdlen == 4 {
                            let mut ip = [0u8; 4];
                            ip.copy_from_slice(rdata);
                            return Ok((ResponseAction::A(ip), hops));
                        }
                    }
                    5 => {
                        // CNAME
                        let (_, cname_target) = parse_name_at(msg, offset, &mut LoopDetector::new())?;

                        // Check for loop in this CNAME chain
                        for visited_name in &visited {
                            if names_equal(visited_name, &cname_target) {
                                return Ok((ResponseAction::CnameLoop, hops));
                            }
                        }

                        hops += 1;
                        if hops > 8 {
                            return Ok((ResponseAction::CnameLoop, hops));
                        }

                        visited.push(cname_target.clone());
                        current = cname_target;
                        found_record = true;
                        break; // Continue the outer loop to look for current
                    }
                    _ => {}
                }
            }

            offset += rdlen;
        }

        if !found_record {
            // No more CNAMEs for current name
            break;
        }
    }

    // No A record found at the end of CNAME chain
    if names_equal(&current, expected_name) {
        // Still at the original name, no records found
        Ok((ResponseAction::NotFound, hops))
    } else {
        // We followed a CNAME but didn't find an A record for the target, need to follow up
        Ok((ResponseAction::CnameTarget(current), hops))
    }
}

struct LoopDetector {
    seen: [u16; 16],
    count: usize,
}

impl LoopDetector {
    fn new() -> Self {
        LoopDetector {
            seen: [0; 16],
            count: 0,
        }
    }

    fn check_and_add(&mut self, offset: u16) -> Result<(), LookupError> {
        // Check for loops (same pointer seen twice)
        for &seen_offset in &self.seen[..self.count] {
            if offset == seen_offset {
                return Err(LookupError::InvalidName); // loop detected
            }
        }

        if self.count < 16 {
            self.seen[self.count] = offset;
            self.count += 1;
        }

        Ok(())
    }
}

/// Parse a name from DNS message, handling compression pointers.
/// Returns (next_offset, parsed_name).
fn parse_name_at(
    msg: &[u8],
    mut offset: usize,
    detector: &mut LoopDetector,
) -> Result<(usize, NameBuf), LookupError> {
    let mut labels = Vec::new();
    let mut first_pointer_next = None;

    loop {
        if offset >= msg.len() {
            return Err(LookupError::InvalidName);
        }

        let len_byte = msg[offset];

        if (len_byte & 0xC0) == 0xC0 {
            // Compression pointer
            if offset + 1 >= msg.len() {
                return Err(LookupError::InvalidName);
            }

            // Remember where to continue after the pointer (only for the first pointer)
            if first_pointer_next.is_none() {
                first_pointer_next = Some(offset + 2);
            }

            let ptr_offset = u16::from_be_bytes([msg[offset], msg[offset + 1]]) & 0x3FFF;
            detector.check_and_add(ptr_offset)?;

            // Move to the pointer target
            offset = ptr_offset as usize;
        } else {
            // Length-prefixed label
            let len = len_byte as usize;

            if len > 63 {
                return Err(LookupError::InvalidName);
            }

            if len == 0 {
                // End of name
                let result_offset = first_pointer_next.unwrap_or(offset + 1);
                return Ok((result_offset, NameBuf { labels }));
            }

            if offset + 1 + len > msg.len() {
                return Err(LookupError::InvalidName);
            }

            labels.push(msg[(offset + 1)..(offset + 1 + len)].to_vec());
            offset += 1 + len;
        }
    }
}

fn names_equal(a: &NameBuf, b: &NameBuf) -> bool {
    if a.labels.len() != b.labels.len() {
        return false;
    }

    for (la, lb) in a.labels.iter().zip(b.labels.iter()) {
        if la.len() != lb.len() {
            return false;
        }

        // Case-insensitive comparison
        for (&ca, &cb) in la.iter().zip(lb.iter()) {
            if !ca.eq_ignore_ascii_case(&cb) {
                return false;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_parse_simple() {
        let n = NameBuf::parse("example.com").unwrap();
        assert_eq!(n.labels.len(), 2);
    }

    #[test]
    fn test_name_parse_trailing_dot() {
        let n = NameBuf::parse("example.com.").unwrap();
        assert_eq!(n.labels.len(), 2);
    }

    #[test]
    fn test_name_invalid_empty() {
        assert!(NameBuf::parse("").is_err());
    }

    #[test]
    fn test_name_invalid_label_too_long() {
        let long = "a".repeat(64);
        assert!(NameBuf::parse(&long).is_err());
    }

    #[test]
    fn test_name_invalid_total_too_long() {
        let long = "a".repeat(63) + "." + &"b".repeat(63) + "." + &"c".repeat(63) + "." + &"d".repeat(62);
        assert!(NameBuf::parse(&long).is_err());
    }

    #[test]
    fn test_name_invalid_bad_char() {
        assert!(NameBuf::parse("exam ple.com").is_err());
        assert!(NameBuf::parse("example.c!om").is_err());
    }

    #[test]
    fn test_name_valid_underscores_hyphens() {
        assert!(NameBuf::parse("_dmarc.example-1.test").is_ok());
    }

    #[test]
    fn test_names_equal_case_insensitive() {
        let a = NameBuf::parse("Example.COM").unwrap();
        let b = NameBuf::parse("example.com").unwrap();
        assert!(names_equal(&a, &b));
    }
}

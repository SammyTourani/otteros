//! Acceptance oracle for brief M5-T1c (a sans-I/O DNS A-record lookup), written by the orchestrator.
//! The crate must pass this file unchanged. Messages are built and parsed here independently.
//!
//! API this file relies on (module `otter_netlogic::dns`):
//!   Lookup::start(name: &str, now_ms: u64, timeout_ms: u64, ids: Box<dyn FnMut() -> u16>)
//!       -> Result<(Lookup, Vec<u8>), LookupError>        // the lookup and its first query message
//!   Lookup::on_response(&mut self, msg: &[u8], now_ms: u64) -> Step   // a datagram from the server
//!   Lookup::poll(&mut self, now_ms: u64) -> Step                      // timers
//!   enum Step { Wait { until_ms: u64 }, Send(Vec<u8>), Done(Result<[u8; 4], LookupError>) }
//!   enum LookupError { InvalidName, NotFound, ServerFailure, Timeout, CnameLoop }
//!     (Step and LookupError: Debug, Clone, PartialEq, Eq; LookupError also Copy)
//!
//! Semantics:
//! - Names: labels of 1-63 bytes from [A-Za-z0-9_-], at most 253 characters, one optional trailing
//!   dot; anything else is InvalidName. A query is a standard DNS message: the id from `ids`, flags
//!   0x0100 (RD), one question (the name's labels as given, QTYPE A = 1, QCLASS IN = 1), no other
//!   records.
//! - Retransmission: the same bytes again at t0+1000, t0+3000, t0+7000, ... (gaps 1, 2, 4, 8 s ...)
//!   where t0 is when that query was first sent; the lookup ends with Timeout at start+timeout_ms.
//!   `Wait { until_ms }` is the earlier of the next retransmission and the deadline.
//! - A response is considered only when its id is the current query's id, QR is set, the opcode is
//!   0, and its single question equals ours (name case-insensitively, type A, class IN); anything
//!   else, and any message that cannot be parsed completely (a name or record running past the end,
//!   a label over 63 bytes, a compression-pointer loop), is ignored: the step is `Wait`.
//! - Considered responses: TC set, or an rcode other than 0 and 3, is ServerFailure; rcode 3 is
//!   NotFound. With rcode 0 the answer section is searched from the question name: an A record
//!   (class IN, rdlength 4) owned by the current name resolves; otherwise a CNAME owned by the
//!   current name moves to its target (at most 8 CNAME hops in the whole lookup, then CnameLoop);
//!   records owned by other names, of other types or other classes are skipped. If the chain ends
//!   without an A record: at the question name that is NotFound (no data); at a CNAME target the
//!   lookup sends a new query for the target with a fresh id from `ids` (Step::Send), and its
//!   retransmission schedule starts over; the deadline does not move.
//! - Owner names compare case-insensitively; compression pointers are followed anywhere in names.
//! - Done is final: every later call returns the same Done.

use otter_netlogic::dns::{Lookup, LookupError, Step};

// ---------------------------------------------------------------------------------------------
// An independent DNS message builder and query parser.
// ---------------------------------------------------------------------------------------------

fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.trim_end_matches('.').split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// A parsed query: (id, flags, qname in its original case, qtype, qclass).
fn parse_query(msg: &[u8]) -> (u16, u16, String, u16, u16) {
    assert!(msg.len() >= 12, "query shorter than a header");
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    let counts: Vec<u16> = (0..4).map(|i| u16::from_be_bytes([msg[4 + 2 * i], msg[5 + 2 * i]])).collect();
    assert_eq!(counts, [1, 0, 0, 0], "one question, no other records");
    let mut i = 12;
    let mut labels = Vec::new();
    loop {
        let n = msg[i] as usize;
        i += 1;
        if n == 0 {
            break;
        }
        assert!(n <= 63, "queries carry plain labels");
        labels.push(String::from_utf8(msg[i..i + n].to_vec()).unwrap());
        i += n;
    }
    let qtype = u16::from_be_bytes([msg[i], msg[i + 1]]);
    let qclass = u16::from_be_bytes([msg[i + 2], msg[i + 3]]);
    assert_eq!(i + 4, msg.len(), "nothing after the question");
    (id, flags, labels.join("."), qtype, qclass)
}

/// A resource record for the builder. `owner` / CNAME targets may be given as a raw encoding
/// (compression pointers included) via `Name::Raw`.
#[derive(Clone)]
enum Name {
    Text(&'static str),
    Raw(Vec<u8>),
}

impl Name {
    fn bytes(&self) -> Vec<u8> {
        match self {
            Name::Text(t) => encode_name(t),
            Name::Raw(r) => r.clone(),
        }
    }
}

#[derive(Clone)]
struct Rr {
    owner: Name,
    rtype: u16,
    class: u16,
    rdata: Vec<u8>,
}

fn a(owner: Name, ip: [u8; 4]) -> Rr {
    Rr { owner, rtype: 1, class: 1, rdata: ip.to_vec() }
}

fn cname(owner: Name, target: Name) -> Rr {
    Rr { owner, rtype: 5, class: 1, rdata: target.bytes() }
}

fn ptr(offset: u16) -> Vec<u8> {
    (0xC000 | offset).to_be_bytes().to_vec()
}

/// A response to `query` (question echoed byte for byte) with the given flags and answers.
fn response_with(query: &[u8], flags: u16, answers: &[Rr]) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&query[0..2]);
    m.extend_from_slice(&flags.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes());
    m.extend_from_slice(&(answers.len() as u16).to_be_bytes());
    m.extend_from_slice(&[0, 0, 0, 0]);
    m.extend_from_slice(&query[12..]);
    for rr in answers {
        m.extend_from_slice(&rr.owner.bytes());
        m.extend_from_slice(&rr.rtype.to_be_bytes());
        m.extend_from_slice(&rr.class.to_be_bytes());
        m.extend_from_slice(&300u32.to_be_bytes());
        m.extend_from_slice(&(rr.rdata.len() as u16).to_be_bytes());
        m.extend_from_slice(&rr.rdata);
    }
    m
}

const OK: u16 = 0x8180; // QR, RD, RA, rcode 0

fn response(query: &[u8], answers: &[Rr]) -> Vec<u8> {
    response_with(query, OK, answers)
}

fn ids(list: &[u16]) -> Box<dyn FnMut() -> u16> {
    let list = list.to_vec();
    let mut next = 0;
    Box::new(move || {
        let id = list[next % list.len()];
        next += 1;
        id
    })
}

fn start(name: &str) -> (Lookup, Vec<u8>) {
    Lookup::start(name, 0, 10_000, ids(&[0x1111, 0x2222, 0x3333, 0x4444])).expect("a valid name")
}

fn done(step: Step) -> Result<[u8; 4], LookupError> {
    match step {
        Step::Done(r) => r,
        other => panic!("expected Done, got {other:?}"),
    }
}

const IP: [u8; 4] = [192, 0, 2, 7];

// ---------------------------------------------------------------------------------------------

#[test]
fn query_is_a_standard_message() {
    let (_l, q) = Lookup::start("www.Otter.test", 0, 5000, ids(&[0xBEEF])).unwrap();
    let mut expected = vec![0xBE, 0xEF, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    expected.extend_from_slice(b"\x03www\x05Otter\x04test\x00\x00\x01\x00\x01");
    assert_eq!(q, expected, "id, RD, one question, the name as given, A, IN");
    let (_l, q2) = Lookup::start("otter.test.", 0, 5000, ids(&[0xBEEF])).unwrap();
    assert_eq!(parse_query(&q2).2, "otter.test", "one trailing dot is allowed and not encoded twice");
}

#[test]
fn invalid_names_are_rejected() {
    let long_label = "a".repeat(64);
    let ok_label = "b".repeat(63);
    let long_name = [ok_label.as_str(), ok_label.as_str(), ok_label.as_str(), &"d".repeat(62)].join(".");
    let max_name = [ok_label.as_str(), ok_label.as_str(), ok_label.as_str(), &"c".repeat(61)].join(".");
    assert_eq!((long_name.len(), max_name.len()), (254, 253), "valid labels; only the total length differs");
    for bad in ["", ".", "a..b", ".a", "a.b..", "bad name", "ünï.test", "a/b", long_label.as_str(), long_name.as_str()] {
        let r = Lookup::start(bad, 0, 5000, ids(&[1]));
        assert!(matches!(r, Err(LookupError::InvalidName)), "{bad:?} must be InvalidName");
    }
    for good in [ok_label.as_str(), max_name.as_str(), "x", "_dmarc.example-1.test", "A-1.B_2"] {
        assert!(Lookup::start(good, 0, 5000, ids(&[1])).is_ok(), "{good:?} is valid");
    }
}

#[test]
fn a_record_resolves() {
    let (mut l, q) = start("otter.test");
    let r = response(&q, &[a(Name::Raw(ptr(12)), IP)]);
    assert_eq!(done(l.on_response(&r, 50)), Ok(IP));
}

#[test]
fn cname_chain_in_one_response_resolves() {
    // www.otter.test CNAME otter.test (the target compressed into the question), then its A record.
    let (mut l, q) = start("www.otter.test");
    let r = response(&q, &[cname(Name::Raw(ptr(12)), Name::Raw(ptr(16))), a(Name::Raw(ptr(16)), IP)]);
    assert_eq!(done(l.on_response(&r, 50)), Ok(IP));

    // Records in any order, other names and types mixed in, a three-hop chain.
    let (mut l, q) = start("a.otter.test");
    let r = response(
        &q,
        &[
            a(Name::Text("evil.test"), [6, 6, 6, 6]),
            a(Name::Text("C.OTTER.TEST"), IP),
            Rr { owner: Name::Text("c.otter.test"), rtype: 28, class: 1, rdata: vec![0; 16] },
            cname(Name::Text("b.otter.test"), Name::Text("c.otter.test")),
            cname(Name::Raw(ptr(12)), Name::Text("b.otter.test")),
        ],
    );
    assert_eq!(done(l.on_response(&r, 50)), Ok(IP), "owner names compare case-insensitively");
}

#[test]
fn cname_without_its_target_sends_a_follow_up_query() {
    let (mut l, q) = start("www.otter.test");
    assert_eq!(parse_query(&q).0, 0x1111);
    let r = response(&q, &[cname(Name::Raw(ptr(12)), Name::Text("cdn.otter.test"))]);
    let q2 = match l.on_response(&r, 400) {
        Step::Send(q2) => q2,
        other => panic!("expected a follow-up query, got {other:?}"),
    };
    let (id2, flags2, name2, t2, c2) = parse_query(&q2);
    assert_eq!((id2, flags2, name2.as_str(), t2, c2), (0x2222, 0x0100, "cdn.otter.test", 1, 1), "fresh id, the target");
    // The first response replayed (old id) is now ignored; the follow-up restarts retransmission.
    assert!(matches!(l.on_response(&r, 450), Step::Wait { .. }));
    assert_eq!(l.poll(1399), Step::Wait { until_ms: 1400 });
    assert_eq!(l.poll(1400), Step::Send(q2.clone()), "retransmitted 1 s after the follow-up was sent");
    let r2 = response(&q2, &[a(Name::Raw(ptr(12)), IP)]);
    assert_eq!(done(l.on_response(&r2, 1500)), Ok(IP));
}

#[test]
fn negative_answers() {
    let (mut l, q) = start("missing.otter.test");
    assert_eq!(done(l.on_response(&response_with(&q, 0x8183, &[]), 10)), Err(LookupError::NotFound), "NXDOMAIN");
    let (mut l, q) = start("empty.otter.test");
    assert_eq!(done(l.on_response(&response(&q, &[]), 10)), Err(LookupError::NotFound), "no data");
    let (mut l, q) = start("other.otter.test");
    let r = response(&q, &[a(Name::Text("elsewhere.test"), IP), Rr { owner: Name::Raw(ptr(12)), rtype: 1, class: 3, rdata: IP.to_vec() }]);
    assert_eq!(done(l.on_response(&r, 10)), Err(LookupError::NotFound), "only records owned by the name, class IN");
    for (flags, what) in [(0x8182u16, "SERVFAIL"), (0x8185, "REFUSED"), (0x8184, "NOTIMP"), (0x8380, "truncated")] {
        let (mut l, q) = start("otter.test");
        let r = response_with(&q, flags, &[a(Name::Raw(ptr(12)), IP)]);
        assert_eq!(done(l.on_response(&r, 10)), Err(LookupError::ServerFailure), "{what}");
    }
}

#[test]
fn responses_that_do_not_match_are_ignored() {
    let (mut l, q) = start("otter.test");
    let good = response(&q, &[a(Name::Raw(ptr(12)), IP)]);
    let mut wrong_id = good.clone();
    wrong_id[1] ^= 1;
    let mut not_response = good.clone();
    not_response[2] &= 0x7F;
    let mut opcode = good.clone();
    opcode[2] |= 0x08; // opcode 1
    let (_o, other_q) = Lookup::start("otter.tesu", 0, 5000, ids(&[0x1111])).unwrap();
    let other_question = response(&other_q, &[a(Name::Raw(ptr(12)), IP)]);
    let mut aaaa_question = good.clone();
    let qtype_at = 12 + encode_name("otter.test").len();
    aaaa_question[qtype_at + 1] = 28;
    for (bad, what) in [(&wrong_id, "id"), (&not_response, "QR clear"), (&opcode, "opcode"), (&other_question, "question name"), (&aaaa_question, "question type")] {
        assert!(matches!(l.on_response(bad, 20), Step::Wait { .. }), "{what} mismatch must be ignored");
    }
    assert_eq!(done(l.on_response(&good, 30)), Ok(IP), "the genuine answer still counts");
}

#[test]
fn malformed_responses_are_ignored() {
    let (mut l, q) = start("otter.test");
    let good = response(&q, &[a(Name::Raw(ptr(12)), IP)]);
    let mut cases: Vec<(Vec<u8>, &str)> = Vec::new();
    for cut in [5, 11, 12, 20, good.len() - 1] {
        cases.push((good[..cut].to_vec(), "truncated"));
    }
    let own_offset = (q.len()) as u16; // the answer's owner points at itself
    cases.push((response(&q, &[a(Name::Raw(ptr(own_offset)), IP)]), "pointer loop"));
    cases.push((response(&q, &[a(Name::Raw(ptr(4000)), IP)]), "pointer past the end"));
    let mut long_label = good.clone();
    long_label[12] = 64;
    cases.push((long_label, "label over 63 bytes"));
    let mut rdlen_past_end = good.clone();
    let n = rdlen_past_end.len();
    rdlen_past_end[n - 5] = 40;
    cases.push((rdlen_past_end, "rdlength past the end"));
    for (bad, what) in &cases {
        assert!(matches!(l.on_response(bad, 20), Step::Wait { .. }), "{what}: must be ignored");
    }
    assert_eq!(l.poll(1000), Step::Send(q.clone()), "still retransmitting");
    // An A record with the wrong length is skipped, not trusted.
    let r = response(&q, &[Rr { owner: Name::Raw(ptr(12)), rtype: 1, class: 1, rdata: vec![1; 16] }, a(Name::Raw(ptr(12)), IP)]);
    assert_eq!(done(l.on_response(&r, 1100)), Ok(IP));
}

#[test]
fn retransmission_and_timeout() {
    let (mut l, q) = Lookup::start("otter.test", 5000, 10_000, ids(&[0x7777])).unwrap();
    assert_eq!(l.poll(5000), Step::Wait { until_ms: 6000 });
    assert_eq!(l.poll(5999), Step::Wait { until_ms: 6000 });
    assert_eq!(l.poll(6000), Step::Send(q.clone()));
    assert_eq!(l.poll(6001), Step::Wait { until_ms: 8000 });
    assert_eq!(l.poll(8000), Step::Send(q.clone()));
    assert_eq!(l.poll(12_000), Step::Send(q.clone()), "at 5000+7000");
    assert_eq!(l.poll(12_001), Step::Wait { until_ms: 15_000 }, "the deadline comes before the next retransmission");
    assert_eq!(l.poll(15_000), Step::Done(Err(LookupError::Timeout)));
    let r = response(&q, &[a(Name::Raw(ptr(12)), IP)]);
    assert_eq!(l.on_response(&r, 15_001), Step::Done(Err(LookupError::Timeout)), "Done is final");
    assert_eq!(l.poll(99_999), Step::Done(Err(LookupError::Timeout)));
}

#[test]
fn late_poll_sends_one_retransmission_not_a_burst() {
    let (mut l, q) = start("otter.test");
    assert_eq!(l.poll(6500), Step::Send(q.clone()), "a poll long after two retransmissions were due");
    assert!(matches!(l.poll(6500), Step::Wait { until_ms } if until_ms > 6500), "and then waits again");
}

#[test]
fn cname_hops_are_limited() {
    let (mut l, q) = start("a.test");
    let r = response(&q, &[cname(Name::Raw(ptr(12)), Name::Text("b.test")), cname(Name::Text("b.test"), Name::Text("a.test"))]);
    assert_eq!(done(l.on_response(&r, 10)), Err(LookupError::CnameLoop), "a loop");

    let names = ["n0.test", "n1.test", "n2.test", "n3.test", "n4.test", "n5.test", "n6.test", "n7.test", "n8.test", "n9.test"];
    let chain = |hops: usize| -> Vec<Rr> {
        let mut rrs: Vec<Rr> = (0..hops).map(|i| cname(Name::Text(names[i]), Name::Text(names[i + 1]))).collect();
        rrs.push(a(Name::Text(names[hops]), IP));
        rrs
    };
    let (mut l, q) = start("n0.test");
    assert_eq!(done(l.on_response(&response(&q, &chain(8)), 10)), Ok(IP), "8 hops are fine");
    let (mut l, q) = start("n0.test");
    assert_eq!(done(l.on_response(&response(&q, &chain(9)), 10)), Err(LookupError::CnameLoop), "9 are not");

    // The limit spans follow-up queries: 5 hops, then 4 more in the next response.
    let (mut l, q) = start("n0.test");
    let first: Vec<Rr> = (0..5).map(|i| cname(Name::Text(names[i]), Name::Text(names[i + 1]))).collect();
    let q2 = match l.on_response(&response(&q, &first), 10) {
        Step::Send(q2) => q2,
        other => panic!("expected a follow-up for n5.test, got {other:?}"),
    };
    assert_eq!(parse_query(&q2).2, "n5.test");
    let second: Vec<Rr> = (5..9).map(|i| cname(Name::Text(names[i]), Name::Text(names[i + 1]))).collect();
    assert_eq!(done(l.on_response(&response(&q2, &second), 20)), Err(LookupError::CnameLoop));
}

#[test]
fn random_corruption_never_panics() {
    let (_l, q) = start("www.otter.test");
    let base = response(&q, &[cname(Name::Raw(ptr(12)), Name::Raw(ptr(16))), a(Name::Raw(ptr(16)), IP)]);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20_000 {
        let (mut l, q) = start("www.otter.test");
        let mut m = base.clone();
        m[..2].copy_from_slice(&q[..2]);
        for _ in 0..1 + next() % 4 {
            let i = (next() as usize) % m.len();
            m[i] = next() as u8;
        }
        if next() % 5 == 0 {
            let len = (next() as usize) % m.len();
            m.truncate(len);
        }
        // Any outcome is acceptable (DNS has no checksum of its own); panics and hangs are not.
        let _ = l.on_response(&m, 10);
        let _ = l.poll(20);
    }
}

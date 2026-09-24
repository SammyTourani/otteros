//! NIST SP 800-90A Rev. 1 §10.1.2 HMAC_DRBG (SHA-256).
//!
//! Implements instantiate, reseed, generate, and zeroise on drop.
//! Reseed interval: 2^20 requests, max 2^16 bytes per request.

use crate::ct::Zeroizing;
use crate::hmac::Hmac;
use crate::sha256::Sha256;
use alloc::vec::Vec;

const RESEED_INTERVAL: u64 = 1u64 << 20; // 2^20 requests
const MAX_BYTES_PER_REQUEST: usize = 1 << 16; // 2^16 bytes

/// Error type for HmacDrbg operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReseedRequired {
    /// The DRBG needs reseeding before generate can produce more output.
    OutOfRequests,
}

/// SP 800-90A HMAC_DRBG with SHA-256.
pub struct HmacDrbg {
    /// Working state V (the output of the HMAC): 32 bytes for SHA-256.
    v: Zeroizing<[u8; 32]>,
    /// HMAC key: 32 bytes for SHA-256.
    key: Zeroizing<[u8; 32]>,
    /// Number of requests (generate calls) since instantiation or last reseed.
    reseed_counter: u64,
}

impl HmacDrbg {
    /// HMAC_Update helper function (SP 800-90A Rev. 1 §10.1.2.2).
    /// Updates Key and V based on provided_data, following the exact spec order.
    fn hmac_update(key: &mut [u8; 32], v: &mut [u8; 32], provided_data: &[u8]) {
        // Step 1: K = HMAC(K, V || 0x00 || data)
        let mut input = Vec::with_capacity(32 + 1 + provided_data.len());
        input.extend_from_slice(&v[..]);
        input.push(0x00);
        input.extend_from_slice(provided_data);
        let digest = Hmac::<Sha256>::mac(&key[..], &input);
        key.copy_from_slice(&digest.as_bytes()[..32]);

        // Step 2: V = HMAC(K, V) -- using the NEW K from step 1 and the ORIGINAL V
        let digest = Hmac::<Sha256>::mac(&key[..], &v[..]);
        v.copy_from_slice(&digest.as_bytes()[..32]);

        // Step 3-5: if data is non-empty, update again
        if !provided_data.is_empty() {
            // Step 4: K = HMAC(K, V || 0x01 || data) -- using the current K and V
            let mut input2 = Vec::with_capacity(32 + 1 + provided_data.len());
            input2.extend_from_slice(&v[..]);
            input2.push(0x01);
            input2.extend_from_slice(provided_data);
            let digest = Hmac::<Sha256>::mac(&key[..], &input2);
            key.copy_from_slice(&digest.as_bytes()[..32]);

            // Step 5: V = HMAC(K, V) -- using the NEW K from step 4 and current V
            let digest = Hmac::<Sha256>::mac(&key[..], &v[..]);
            v.copy_from_slice(&digest.as_bytes()[..32]);
        }
    }

    /// Instantiates the DRBG from entropy, optional nonce, and optional personalization.
    /// SP 800-90A §10.1.2.3.
    pub fn instantiate(entropy: &[u8], nonce: &[u8], personalization: &[u8]) -> Self {
        // Step 1: seed_material = entropy || nonce || personalization
        let mut seed_material = Vec::with_capacity(entropy.len() + nonce.len() + personalization.len());
        seed_material.extend_from_slice(entropy);
        seed_material.extend_from_slice(nonce);
        seed_material.extend_from_slice(personalization);

        // Step 2: Key = 0x00 * outlen
        let mut key = [0u8; 32];
        // Step 3: V = 0x01 * outlen
        let mut v = [1u8; 32];

        // Step 4: (Key, V) = HMAC_Update(Key, V, seed_material)
        Self::hmac_update(&mut key, &mut v, &seed_material);

        HmacDrbg {
            v: Zeroizing::new(v),
            key: Zeroizing::new(key),
            reseed_counter: 1,
        }
    }

    /// Reseeds the DRBG with new entropy and optional additional input.
    /// SP 800-90A §10.1.2.4.
    pub fn reseed(&mut self, entropy: &[u8], additional: &[u8]) {
        // Step 1: seed_material = entropy || additional
        let mut seed_material = Vec::with_capacity(entropy.len() + additional.len());
        seed_material.extend_from_slice(entropy);
        seed_material.extend_from_slice(additional);

        // Step 2: (Key, V) = HMAC_Update(Key, V, seed_material)
        // Extract from Zeroizing, update, and re-wrap
        let mut key_array = self.key.get();
        let mut v_array = self.v.get();
        Self::hmac_update(&mut key_array, &mut v_array, &seed_material);
        self.key = Zeroizing::new(key_array);
        self.v = Zeroizing::new(v_array);

        // Step 3: reseed_counter = 1
        self.reseed_counter = 1;
    }

    /// Generates random bytes into `out` with optional additional input.
    /// Returns `Ok(())` if generation succeeds, or `ReseedRequired` if the
    /// reseed interval (2^20 requests) has been exceeded.
    /// SP 800-90A §10.1.2.5.
    pub fn generate(&mut self, out: &mut [u8], additional: &[u8]) -> Result<(), ReseedRequired> {
        // Check reseed counter
        if self.reseed_counter > RESEED_INTERVAL {
            return Err(ReseedRequired::OutOfRequests);
        }

        // Check maximum bytes per request
        if out.len() > MAX_BYTES_PER_REQUEST {
            return Err(ReseedRequired::OutOfRequests);
        }

        // Step 1: If additional input is provided, update Key and V
        if !additional.is_empty() {
            let mut key_array = self.key.get();
            let mut v_array = self.v.get();
            Self::hmac_update(&mut key_array, &mut v_array, additional);
            self.key = Zeroizing::new(key_array);
            self.v = Zeroizing::new(v_array);
        }

        // Step 2: Generate requested bits
        // While len(temp) < requested_number_of_bits:
        //   a. V = HMAC(K, V)
        //   b. temp = temp || V
        let mut generated = 0;
        while generated < out.len() {
            // Step 2a: V = HMAC(K, V)
            let digest = Hmac::<Sha256>::mac(&self.key[..], &*self.v);
            let mut v_array = self.v.get();
            v_array.copy_from_slice(&digest.as_bytes()[..32]);
            self.v = Zeroizing::new(v_array);

            // Step 2b: temp = temp || V (append V to output)
            let chunk_size = (out.len() - generated).min(32);
            out[generated..generated + chunk_size].copy_from_slice(&digest.as_bytes()[..chunk_size]);
            generated += chunk_size;
        }

        // Step 3: Update Key and V
        let mut key_array = self.key.get();
        let mut v_array = self.v.get();
        Self::hmac_update(&mut key_array, &mut v_array, additional);
        self.key = Zeroizing::new(key_array);
        self.v = Zeroizing::new(v_array);

        self.reseed_counter += 1;
        Ok(())
    }

    /// For testing only: expose the internal state for comparison against CAVP vectors.
    #[doc(hidden)]
    pub fn test_state(&self) -> (&[u8; 32], &[u8; 32]) {
        (&self.key, &self.v)
    }
}

impl Drop for HmacDrbg {
    fn drop(&mut self) {
        // The Zeroizing guard handles zeroing on drop.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
    }

    #[test]
    fn cavp_hmac_drbg_basic() {
        // Simple test case from the NIST vectors.
        // SHA-256, no personalization, no additional input.
        // Per CAVP spec: instantiate -> reseed -> generate (discard) -> generate -> compare

        let entropy = hex_decode("06032cd5eed33f39265f49ecb142c511da9aff2af71203bffaf34a9ca5bd9c0d");
        let nonce = hex_decode("0e66f71edc43e42a45ad3c6fc6cdc4df");

        // First instantiate
        let mut drbg = HmacDrbg::instantiate(&entropy, &nonce, &[]);

        // Reseed with the given reseed entropy
        let reseed_entropy = hex_decode("01920a4e669ed3a85ae8a33b35a74ad7fb2a6bb4cf395ce00334a9c9a5a5d552");
        drbg.reseed(&reseed_entropy, &[]);

        // First generate - discard output
        let mut discard = [0u8; 128];
        drbg.generate(&mut discard, &[]).unwrap();

        // Second generate - compare this output
        let mut output = [0u8; 128];
        drbg.generate(&mut output, &[]).unwrap();

        let expected = "76fc79fe9b50beccc991a11b5635783a83536add03c157fb30645e611c2898bb\
                       2b1bc215000209208cd506cb28da2a51bdb03826aaf2bd2335d576d519160842\
                       e7158ad0949d1a9ec3e66ea1b1a064b005de914eac2e9d4f2d72a8616a80225\
                       422918250ff66a41bd2f864a6a38cc5b6499dc43f7f2bd09e1e0f8f5885935124";

        let expected_clean = expected.replace(['\n', ' '], "");
        assert_eq!(hex(&output), expected_clean,
                   "First generate call mismatch");
    }
}

//! H.264 Annex-B helpers for the warm-decoder spike.
//!
//! Why this file exists: mp4 stores H.264 video as length-prefixed NAL units
//! ("AVCC"), while FFmpeg's raw H.264 input (`-f h264`) expects start-code-
//! prefixed NAL units ("Annex-B"). A warm decoder fed over stdin must convert
//! before writing. The SPS/PPS parameter sets live in the mp4's `avcC` record
//! (fetched via ffprobe) and are re-sent in-band before every keyframe so the
//! decoder can resynchronise after a jump.
//!
//! Everything here is pure byte arithmetic against the documented layouts —
//! no decoder knowledge, no unsafe, no dependencies.

/// The four bytes that begin every Annex-B NAL unit in our output.
pub const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

/// Access Unit Delimiter as Annex-B bytes: NAL type 9, `primary_pic_type = 7`
/// ("any slice type"), then `rbsp_trailing_bits`.
///
/// The H.264 parser closes an access unit when it meets the next unit's slice
/// header — or, if there is no next unit yet, when it meets an AUD. On a
/// stdin stream that "next unit" may be a whole feed away, so without an AUD
/// the decoder sits on complete pictures. Measured in the protocol probe:
/// an AUD per packet raised decoder output from 3/8 to 6/8 frames per burst
/// and made per-jump emission deterministic.
pub const AUD: [u8; 6] = [0x00, 0x00, 0x00, 0x01, 0x09, 0xF0];

/// Parsed subset of the AVCDecoderConfigurationRecord (`avcC`): everything
/// the warm feeder needs. Field layout is ISO/IEC 14496-15 §5.3.3.1.
pub struct AvcConfig {
    /// Bytes per NAL length prefix in mp4 samples (usually 4).
    pub nal_length_size: usize,
    /// Sequence parameter sets (usually exactly one).
    pub sps: Vec<Vec<u8>>,
    /// Picture parameter sets (usually exactly one).
    pub pps: Vec<Vec<u8>>,
}

/// Parses the `avcC` payload into the piece the feeder cares about.
/// Rejects truncated or nonsensical records loudly instead of guessing —
/// a wrong length here would silently feed garbage to the decoder.
pub fn parse_avc_config(record: &[u8]) -> Result<AvcConfig, String> {
    if record.len() < 7 {
        return Err(format!(
            "avcC record too short: {} bytes (need at least 7)",
            record.len()
        ));
    }
    if record[0] != 1 {
        return Err(format!("avcC version {} is not the expected 1", record[0]));
    }
    // Byte 4 low 2 bits = lengthSizeMinusOne; byte 5 low 5 bits = SPS count.
    let nal_length_size = usize::from(record[4] & 0x03) + 1;
    let num_sps = usize::from(record[5] & 0x1F);
    if num_sps == 0 {
        return Err("avcC declares zero SPS entries".to_string());
    }
    let mut cur = 6usize;
    let mut sps = Vec::with_capacity(num_sps);
    for _ in 0..num_sps {
        if record.len() < cur + 2 {
            return Err("avcC truncated in an SPS length field".to_string());
        }
        let len = usize::from(u16::from_be_bytes([record[cur], record[cur + 1]]));
        cur += 2;
        if len == 0 || record.len() < cur + len {
            return Err("avcC truncated (or zero-length) inside an SPS".to_string());
        }
        sps.push(record[cur..cur + len].to_vec());
        cur += len;
    }
    if record.len() <= cur {
        return Err("avcC missing the PPS count byte".to_string());
    }
    let num_pps = usize::from(record[cur]);
    cur += 1;
    if num_pps == 0 {
        return Err("avcC declares zero PPS entries".to_string());
    }
    let mut pps = Vec::with_capacity(num_pps);
    for _ in 0..num_pps {
        if record.len() < cur + 2 {
            return Err("avcC truncated in a PPS length field".to_string());
        }
        let len = usize::from(u16::from_be_bytes([record[cur], record[cur + 1]]));
        cur += 2;
        if len == 0 || record.len() < cur + len {
            return Err("avcC truncated (or zero-length) inside a PPS".to_string());
        }
        pps.push(record[cur..cur + len].to_vec());
        cur += len;
    }
    Ok(AvcConfig {
        nal_length_size,
        sps,
        pps,
    })
}

/// The parameter sets as one Annex-B byte block: every SPS then every PPS,
/// each preceded by a start code. Written to the decoder before each keyframe
/// so a jump can re-initialise it from cold at any point in the stream.
pub fn param_sets_annexb(config: &AvcConfig) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in config.sps.iter().chain(config.pps.iter()) {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
    }
    out
}

/// Converts one mp4 video sample (length-prefixed NAL units) to Annex-B,
/// appending to `out`. Returns an error on any malformed prefix/length —
/// malformed input is surfaced, never skipped (the spike must be able to
/// trust every byte it feeds the decoder).
pub fn avcc_to_annexb(
    sample: &[u8],
    nal_length_size: usize,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if !(1..=4).contains(&nal_length_size) {
        return Err(format!(
            "invalid NAL length size {nal_length_size} (must be 1..=4)"
        ));
    }
    let mut cur = 0usize;
    while cur < sample.len() {
        if sample.len() - cur < nal_length_size {
            return Err(format!(
                "sample ends with {} trailing bytes lacking a full {nal_length_size}-byte length prefix",
                sample.len() - cur
            ));
        }
        let mut len = 0usize;
        for &b in &sample[cur..cur + nal_length_size] {
            len = (len << 8) | usize::from(b);
        }
        cur += nal_length_size;
        if len == 0 {
            return Err("sample contains a zero-length NAL unit".to_string());
        }
        if sample.len() - cur < len {
            return Err(format!(
                "NAL length {len} exceeds the {} remaining sample bytes",
                sample.len() - cur
            ));
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&sample[cur..cur + len]);
        cur += len;
    }
    Ok(())
}

/// Appends one packet to a decoder feed: the parameter sets, an AUD, then the
/// packet's NAL units in Annex-B form. One mp4 packet is exactly one access
/// unit, so the AUD marks where this packet's picture ends and lets the
/// decoder emit it without waiting for the next packet.
pub fn append_access_unit(
    out: &mut Vec<u8>,
    param_sets: &[u8],
    sample: &[u8],
    nal_length_size: usize,
) -> Result<(), String> {
    out.extend_from_slice(param_sets);
    out.extend_from_slice(&AUD);
    avcc_to_annexb(sample, nal_length_size, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real avcC payload ffprobe reported for our test clip (48 bytes:
    /// High profile, 4-byte lengths, one 27-byte SPS, one 6-byte PPS).
    const REAL_AVCC: [u8; 48] = [
        0x01, 0x64, 0x00, 0x28, 0xFF, 0xE1, 0x00, 0x1B, 0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9, 0x40,
        0x78, 0x02, 0x27, 0xE5, 0xC0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00,
        0xF0, 0x3C, 0x60, 0xC6, 0x58, 0x01, 0x00, 0x06, 0x68, 0xEB, 0xE2, 0x4B, 0x22, 0xC0, 0xFD,
        0xF8, 0xF8, 0x00,
    ];

    #[test]
    fn parses_the_real_extradata_record() {
        let cfg = parse_avc_config(&REAL_AVCC).expect("real record must parse");
        assert_eq!(cfg.nal_length_size, 4);
        assert_eq!(cfg.sps.len(), 1);
        assert_eq!(cfg.pps.len(), 1);
        assert_eq!(cfg.sps[0].len(), 27);
        assert_eq!(cfg.sps[0][0], 0x67, "SPS NAL type must lead the SPS");
        assert_eq!(cfg.pps[0], [0x68, 0xEB, 0xE2, 0x4B, 0x22, 0xC0]);
    }

    #[test]
    fn rejects_truncated_and_nonsense_records() {
        assert!(parse_avc_config(&[]).is_err());
        assert!(parse_avc_config(&[0x02; 8]).is_err(), "bad version");
        assert!(parse_avc_config(&REAL_AVCC[..30]).is_err(), "truncated SPS");
        let mut no_sps = REAL_AVCC;
        no_sps[5] &= 0xE0; // numSPS = 0
        assert!(parse_avc_config(&no_sps).is_err());
    }

    #[test]
    fn converts_length_prefixed_nals_to_annexb() {
        // Two NALs: [aa bb] and [cc dd ee], 2-byte length prefixes.
        let sample = [0x00, 0x02, 0xAA, 0xBB, 0x00, 0x03, 0xCC, 0xDD, 0xEE];
        let mut out = Vec::new();
        avcc_to_annexb(&sample, 2, &mut out).expect("well-formed sample");
        assert_eq!(
            out,
            [0x00, 0x00, 0x00, 0x01, 0xAA, 0xBB, 0x00, 0x00, 0x00, 0x01, 0xCC, 0xDD, 0xEE]
        );
    }

    #[test]
    fn rejects_malformed_samples() {
        let mut out = Vec::new();
        assert!(
            avcc_to_annexb(&[0x00], 2, &mut out).is_err(),
            "partial prefix"
        );
        assert!(
            avcc_to_annexb(&[0x00, 0x05, 0xAA], 2, &mut out).is_err(),
            "overlong NAL"
        );
        assert!(
            avcc_to_annexb(&[0x00, 0x00], 2, &mut out).is_err(),
            "zero-length NAL"
        );
        assert!(
            avcc_to_annexb(&[0xAA], 0, &mut out).is_err(),
            "bad prefix size"
        );
    }

    #[test]
    fn param_sets_are_start_code_prefixed() {
        let cfg = parse_avc_config(&REAL_AVCC).expect("real record");
        let block = param_sets_annexb(&cfg);
        assert_eq!(&block[..4], &START_CODE);
        assert_eq!(&block[4..4 + 27], &cfg.sps[0]);
        assert_eq!(&block[4 + 27..8 + 27], &START_CODE);
        assert_eq!(&block[8 + 27..], &cfg.pps[0][..]);
    }

    #[test]
    fn one_feed_packet_is_params_then_aud_then_sample() {
        assert_eq!(AUD[4] & 0x1F, 9, "AUD must be NAL type 9");
        let cfg = parse_avc_config(&REAL_AVCC).expect("real record");
        let params = param_sets_annexb(&cfg);
        let sample = [0x00, 0x02, 0xAA, 0xBB];

        let mut out = Vec::new();
        append_access_unit(&mut out, &params, &sample, 2).expect("well-formed sample");

        let mut expected = params;
        expected.extend_from_slice(&AUD);
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0xAA, 0xBB]);
        assert_eq!(out, expected);
    }
}

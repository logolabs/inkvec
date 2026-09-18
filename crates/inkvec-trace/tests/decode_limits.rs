//! Hostile image input is refused with an error, never a panic or an allocation the size of
//! the dimensions a file claims.
//!
//! The protection is the `image` crate's default allocation limit, not code in this crate.
//! These tests pin it, so a dependency upgrade or a new decode path cannot quietly remove it.

use inkvec_trace::decode_image;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A zlib stream holding `raw` in one uncompressed block (fine for the few bytes used here).
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    assert!(raw.len() < 65_536);
    let len = raw.len() as u16;
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&len.to_le_bytes());
    z.extend_from_slice(&(!len).to_le_bytes());
    z.extend_from_slice(raw);
    z.extend_from_slice(&adler32(raw).to_be_bytes());
    z
}

/// A structurally valid RGBA PNG whose header claims `w x h` pixels, carrying `scanlines` of
/// image data (which may be far less than the header promises).
fn png(w: u32, h: u32, scanlines: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(scanlines));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn the_png_builder_makes_a_file_the_decoder_accepts() {
    // Guards the refusal below: without this, it could be refusing a malformed file rather
    // than an oversized one. One scanline: filter byte 0, then one red, opaque pixel.
    let img = decode_image(&png(1, 1, &[0, 255, 0, 0, 255])).expect("a valid 1x1 PNG decodes");
    assert_eq!((img.width, img.height), (1, 1));
    assert!((img.data[0] - 1.0).abs() < 1e-6 && img.data[1].abs() < 1e-6);
}

#[test]
fn a_png_claiming_ten_billion_pixels_is_refused_without_allocating() {
    let err =
        decode_image(&png(100_000, 100_000, &[])).expect_err("a 100000x100000 claim is refused");
    assert!(!err.to_string().is_empty());
}

#[test]
fn empty_and_garbage_bytes_are_errors() {
    assert!(decode_image(&[]).is_err());
    assert!(decode_image(b"not an image at all").is_err());
    assert!(decode_image(&png(4, 4, &[])[..20]).is_err());
}

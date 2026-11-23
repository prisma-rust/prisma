use super::*;

#[test]
fn rgbe_zero_exponent_is_black() {
    let (r, g, b) = rgbe_to_float(255.0, 255.0, 255.0, 0.0);
    assert_eq!((r, g, b), (0.0, 0.0, 0.0));
}

#[test]
fn rgbe_known_value() {
    let (r, _, _) = rgbe_to_float(128.0, 0.0, 0.0, 128.0);
    assert!((r - 0.5).abs() < 1e-4);
}

#[test]
fn decode_rle_scanline() {
    let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 4\n";
    let scanline = [
        0x02u8, 0x02, 0x00, 0x04, 0x84, 0x0a, 0x84, 0x14, 0x84, 0x1e, 0x84, 0x80,
    ];
    let mut data = header.to_vec();
    data.extend_from_slice(&scanline);

    let (rgba, w, h) = load_rgbe(&data).expect("decode rle");
    assert_eq!((w, h), (4, 1));
    assert_eq!(rgba.len(), 16);
    assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
    assert!((rgba[1] - 20.0 / 256.0).abs() < 1e-5);
    assert!((rgba[2] - 30.0 / 256.0).abs() < 1e-5);
    assert_eq!(rgba[3], 1.0);
}

#[test]
fn decode_rle_literal_run() {
    let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 4\n";
    let scanline = [
        0x02u8, 0x02, 0x00, 0x04, 0x04, 0x0a, 0x14, 0x1e, 0x28, 0x04, 0x01, 0x02, 0x03, 0x04, 0x04,
        0x05, 0x06, 0x07, 0x08, 0x04, 0x80, 0x80, 0x80, 0x80,
    ];
    let mut data = header.to_vec();
    data.extend_from_slice(&scanline);

    let (rgba, w, h) = load_rgbe(&data).expect("decode rle literal");
    assert_eq!((w, h), (4, 1));
    assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
    assert!((rgba[1] - 1.0 / 256.0).abs() < 1e-5);
    assert!((rgba[2] - 5.0 / 256.0).abs() < 1e-5);
    assert_eq!(rgba[3], 1.0);
}

#[test]
fn rejects_empty_and_unterminated_header() {
    assert!(load_rgbe(b"").is_err());
    assert!(load_rgbe(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n").is_err());
}

#[test]
fn rejects_truncated_rle_scanline() {
    let header = b"-Y 1 +X 4\n";
    let mut data = header.to_vec();
    data.extend_from_slice(&[0x02, 0x02, 0x00, 0x04]);
    assert!(load_rgbe(&data).is_err());
}

#[test]
fn rejects_zero_length_rle_run() {
    let header = b"-Y 1 +X 4\n";
    let mut data = header.to_vec();
    data.extend_from_slice(&[0x02, 0x02, 0x00, 0x04, 0x00]);
    assert!(load_rgbe(&data).is_err());
}

#[test]
fn rejects_rle_run_past_scanline() {
    let header = b"-Y 1 +X 4\n";
    let mut data = header.to_vec();
    data.extend_from_slice(&[0x02, 0x02, 0x00, 0x04, 0x85, 0x0a]);
    assert!(load_rgbe(&data).is_err());
}

#[test]
fn rejects_truncated_uncompressed_scanline() {
    let header = b"-Y 1 +X 4\n";
    let mut data = header.to_vec();
    data.extend_from_slice(&[1, 2, 3]);
    assert!(load_rgbe(&data).is_err());
}

#[test]
fn rejects_overflowing_dimensions() {
    assert!(load_rgbe(b"-Y 4294967295 +X 4294967295\n").is_err());
}

#[test]
fn decodes_uncompressed_scanline() {
    let header = b"-Y 1 +X 2\n";
    let mut data = header.to_vec();
    data.extend_from_slice(&[128, 64, 32, 128, 0, 0, 0, 0]);
    let (rgba, w, h) = load_rgbe(&data).expect("decode uncompressed");
    assert_eq!((w, h), (2, 1));
    assert!((rgba[0] - 0.5).abs() < 1e-4);
    assert!((rgba[1] - 0.25).abs() < 1e-4);
    assert!((rgba[2] - 0.125).abs() < 1e-4);
    assert_eq!(rgba[3], 1.0);
}

/// RGBE bytes for one pixel with exposure 128 (so the float scale is 1/256).
fn rgbe_pixel(r: u8) -> [u8; 4] {
    [r, 0, 0, 128]
}

#[test]
fn flips_rows_for_positive_y() {
    let mut data = b"+Y 2 +X 2\n".to_vec();
    for r in [10u8, 20, 30, 40] {
        data.extend_from_slice(&rgbe_pixel(r));
    }

    let (rgba, w, h) = load_rgbe(&data).expect("decode +Y");
    assert_eq!((w, h), (2, 2));
    assert!((rgba[0] - 30.0 / 256.0).abs() < 1e-5);
    assert!((rgba[4] - 40.0 / 256.0).abs() < 1e-5);
    assert!((rgba[8] - 10.0 / 256.0).abs() < 1e-5);
    assert!((rgba[12] - 20.0 / 256.0).abs() < 1e-5);
}

#[test]
fn flips_columns_for_negative_x() {
    let mut data = b"-Y 1 -X 2\n".to_vec();
    data.extend_from_slice(&rgbe_pixel(10));
    data.extend_from_slice(&rgbe_pixel(20));

    let (rgba, w, h) = load_rgbe(&data).expect("decode -X");
    assert_eq!((w, h), (2, 1));
    assert!((rgba[0] - 20.0 / 256.0).abs() < 1e-5);
    assert!((rgba[4] - 10.0 / 256.0).abs() < 1e-5);
}

#[test]
fn decodes_transposed_axes() {
    let mut data = b"+X 2 -Y 2\n".to_vec();
    for r in [10u8, 20, 30, 40] {
        data.extend_from_slice(&rgbe_pixel(r));
    }

    let (rgba, w, h) = load_rgbe(&data).expect("decode transposed");
    assert_eq!((w, h), (2, 2));
    assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
    assert!((rgba[4] - 30.0 / 256.0).abs() < 1e-5);
    assert!((rgba[8] - 20.0 / 256.0).abs() < 1e-5);
    assert!((rgba[12] - 40.0 / 256.0).abs() < 1e-5);
}

#[test]
fn decodes_rle_scanline_with_non_standard_orientation() {
    let mut data = b"+Y 1 +X 4\n".to_vec();
    data.extend_from_slice(&[
        0x02, 0x02, 0x00, 0x04, 0x84, 0x0a, 0x84, 0x14, 0x84, 0x1e, 0x84, 0x80,
    ]);

    let (rgba, w, h) = load_rgbe(&data).expect("decode rle +Y");
    assert_eq!((w, h), (4, 1));
    assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
    assert_eq!(rgba[3], 1.0);
}

#[test]
fn rejects_repeated_axis() {
    let mut data = b"-Y 2 -Y 2\n".to_vec();
    data.extend_from_slice(&[0u8; 16]);
    assert!(load_rgbe(&data).is_err());
}

#[test]
fn rejects_zero_dimension() {
    assert!(load_rgbe(b"-Y 0 +X 4\n").is_err());
    assert!(load_rgbe(b"-Y 4 +X 0\n").is_err());
}

#[test]
fn rejects_unparsable_resolution_length() {
    assert!(load_rgbe(b"-Y four +X 4\n").is_err());
    assert!(load_rgbe(b"-Y 4\n").is_err());
}

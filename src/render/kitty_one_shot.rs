//! Query-free Kitty Graphics Protocol serialization for one-shot output.

use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const ESC: u8 = 0x1b;
const MAX_CHUNK: usize = 4096;
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

// Ordered from kitty's official rowcolumn-diacritics.txt, first 256 entries.
const DIACRITICS: [u32; 256] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerializedImage {
    pub transfer: Vec<u8>,
    pub placeholder_rows: Vec<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerializeError {
    EmptyPayload,
    InvalidGeometry,
    Overflow,
}

pub fn next_image_id() -> NonZeroU32 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u32);
    let mixed = NEXT_ID.fetch_add(0x9e37_79b9, Ordering::Relaxed) ^ time ^ std::process::id();
    NonZeroU32::new(mixed).unwrap_or_else(|| NonZeroU32::new(1).unwrap())
}

pub fn serialize(
    png: &[u8],
    id: NonZeroU32,
    cols: u16,
    rows: u16,
    tmux: bool,
) -> Result<SerializedImage, SerializeError> {
    if png.is_empty() {
        return Err(SerializeError::EmptyPayload);
    }
    if cols == 0 || rows == 0 || cols > 256 || rows > 256 {
        return Err(SerializeError::InvalidGeometry);
    }
    let encoded = base64(png)?;
    let mut transfer = Vec::new();
    for (index, chunk) in encoded.chunks(MAX_CHUNK).enumerate() {
        let more = usize::from((index + 1) * MAX_CHUNK < encoded.len());
        let control = if index == 0 {
            format!(
                "a=T,t=d,f=100,U=1,i={},c={cols},r={rows},q=2,N=1,m={more}",
                id
            )
        } else {
            format!("m={more},q=2")
        };
        let mut apc = b"\x1b_G".to_vec();
        apc.extend(control.bytes());
        apc.push(b';');
        apc.extend(chunk);
        apc.extend(b"\x1b\\");
        if tmux {
            transfer.extend(wrap_tmux(&apc));
        } else {
            transfer.extend(apc);
        }
    }
    let color = id.get() & 0x00ff_ffff;
    let high = (id.get() >> 24) as usize;
    let mut placeholder_rows = Vec::with_capacity(rows.into());
    for &row_diacritic in DIACRITICS.iter().take(usize::from(rows)) {
        let mut line = format!(
            "\x1b[38;2;{};{};{}m",
            color >> 16,
            (color >> 8) & 255,
            color & 255
        );
        for &column_diacritic in DIACRITICS.iter().take(usize::from(cols)) {
            line.push('\u{10eeee}');
            for value in [row_diacritic, column_diacritic, DIACRITICS[high]] {
                line.push(char::from_u32(value).ok_or(SerializeError::Overflow)?);
            }
        }
        line.push_str("\x1b[0m\n");
        placeholder_rows.push(line);
    }
    Ok(SerializedImage {
        transfer,
        placeholder_rows,
    })
}

fn wrap_tmux(apc: &[u8]) -> Vec<u8> {
    let mut out = b"\x1bPtmux;".to_vec();
    for &b in apc {
        out.push(b);
        if b == ESC {
            out.push(ESC);
        }
    }
    out.extend(b"\x1b\\");
    out
}
fn base64(input: &[u8]) -> Result<Vec<u8>, SerializeError> {
    let size = input
        .len()
        .checked_add(2)
        .ok_or(SerializeError::Overflow)?
        .checked_div(3)
        .ok_or(SerializeError::Overflow)?
        .checked_mul(4)
        .ok_or(SerializeError::Overflow)?;
    let mut out = Vec::with_capacity(size);
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.extend([
            A[(n >> 18) as usize],
            A[((n >> 12) & 63) as usize],
            if chunk.len() > 1 {
                A[((n >> 6) & 63) as usize]
            } else {
                b'='
            },
            if chunk.len() > 2 {
                A[(n & 63) as usize]
            } else {
                b'='
            },
        ]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct ParsedCommand {
        control: Vec<u8>,
        payload: Vec<u8>,
    }

    fn parse_apc_stream(stream: &[u8]) -> Vec<ParsedCommand> {
        let mut commands = Vec::new();
        let mut offset = 0;
        while offset < stream.len() {
            assert!(stream.len() - offset >= 3, "truncated APC start");
            assert_eq!(&stream[offset..offset + 3], b"\x1b_G");

            let body_start = offset + 3;
            let terminator = stream[body_start..]
                .windows(2)
                .position(|window| window == b"\x1b\\")
                .expect("APC terminator");
            let body_end = body_start + terminator;
            let separator = stream[body_start..body_end]
                .iter()
                .position(|byte| *byte == b';')
                .expect("APC control/payload separator");
            let separator = body_start + separator;
            let control = stream[body_start..separator].to_vec();
            let payload = stream[separator + 1..body_end].to_vec();
            assert!(!payload.contains(&ESC), "payload contains an escape");
            assert!(!payload.contains(&b';'), "payload contains control data");
            assert!(payload.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/' | b'=')
            }));
            commands.push(ParsedCommand { control, payload });
            offset = body_end + 2;
        }
        commands
    }

    fn frame_apc(command: &ParsedCommand) -> Vec<u8> {
        let mut frame = b"\x1b_G".to_vec();
        frame.extend(&command.control);
        frame.push(b';');
        frame.extend(&command.payload);
        frame.extend(b"\x1b\\");
        frame
    }

    fn control_fields(control: &[u8]) -> Vec<&[u8]> {
        control.split(|byte| *byte == b',').collect()
    }

    fn base64_value(byte: u8) -> u8 {
        match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid base64 byte: {byte:?}"),
        }
    }

    fn decode_base64(encoded: &[u8]) -> Vec<u8> {
        assert_eq!(encoded.len() % 4, 0, "base64 length");
        let chunk_count = encoded.len() / 4;
        let mut decoded = Vec::new();
        for (index, chunk) in encoded.chunks(4).enumerate() {
            assert!(!chunk[..2].contains(&b'='));
            let first = base64_value(chunk[0]);
            let second = base64_value(chunk[1]);
            decoded.push((first << 2) | (second >> 4));

            if chunk[2] == b'=' {
                assert_eq!(chunk[3], b'=');
                assert_eq!(index + 1, chunk_count);
                continue;
            }
            let third = base64_value(chunk[2]);
            decoded.push((second << 4) | (third >> 2));

            if chunk[3] == b'=' {
                assert_eq!(index + 1, chunk_count);
            } else {
                let fourth = base64_value(chunk[3]);
                decoded.push((third << 6) | fourth);
            }
        }
        decoded
    }

    fn parse_tmux_stream(stream: &[u8]) -> Vec<Vec<u8>> {
        let prefix = b"\x1bPtmux;";
        let mut commands = Vec::new();
        let mut offset = 0;
        while offset < stream.len() {
            assert!(
                stream.len() - offset >= prefix.len(),
                "truncated tmux wrapper"
            );
            assert_eq!(&stream[offset..offset + prefix.len()], prefix);
            let body_start = offset + prefix.len();
            let mut cursor = body_start;
            let mut inner = Vec::new();
            loop {
                assert!(cursor < stream.len(), "unterminated tmux wrapper");
                if stream[cursor] != ESC {
                    inner.push(stream[cursor]);
                    cursor += 1;
                    continue;
                }
                assert!(cursor + 1 < stream.len(), "truncated tmux escape");
                match stream[cursor + 1] {
                    ESC => {
                        inner.push(ESC);
                        cursor += 2;
                    }
                    b'\\' => {
                        cursor += 2;
                        break;
                    }
                    byte => panic!("unexpected tmux escape: {byte:?}"),
                }
            }
            assert!(
                !stream[body_start..cursor]
                    .windows(3)
                    .any(|window| window == [ESC, ESC, ESC])
            );
            assert_eq!(parse_apc_stream(&inner).len(), 1);
            commands.push(inner);
            offset = cursor;
        }
        commands
    }

    fn expected_placeholder_row(id: u32, row: usize, cols: usize) -> String {
        let color = id & 0x00ff_ffff;
        let mut line = format!(
            "\x1b[38;2;{};{};{}m",
            color >> 16,
            (color >> 8) & 255,
            color & 255
        );
        for &column_diacritic in DIACRITICS.iter().take(cols) {
            line.push('\u{10eeee}');
            for value in [
                DIACRITICS[row],
                column_diacritic,
                DIACRITICS[(id >> 24) as usize],
            ] {
                line.push(char::from_u32(value).unwrap());
            }
        }
        line.push_str("\x1b[0m\n");
        line
    }

    fn assert_exact_placeholder_rows(
        serialized: &SerializedImage,
        id: NonZeroU32,
        cols: usize,
        rows: usize,
    ) {
        assert_eq!(serialized.placeholder_rows.len(), rows);
        for (row, actual) in serialized.placeholder_rows.iter().enumerate() {
            assert_eq!(actual, &expected_placeholder_row(id.get(), row, cols));
            assert_eq!(actual.matches('\u{10eeee}').count(), cols);
            assert!(actual.ends_with("\x1b[0m\n"));
        }
    }

    fn assert_no_forbidden_terminal_sequences(bytes: &[u8]) {
        for forbidden in [
            b"\x1b[5n".as_slice(),
            b"\x1b[6n".as_slice(),
            b"\x1b[14t".as_slice(),
            b"\x1b[16t".as_slice(),
            b"\x1b[s".as_slice(),
            b"\x1b[u".as_slice(),
            b"\x1b7".as_slice(),
            b"\x1b8".as_slice(),
            b"?1049".as_slice(),
        ] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden)
            );
        }

        let mut offset = 0;
        while let Some(relative) = bytes[offset..]
            .windows(2)
            .position(|window| window == [ESC, b'['])
        {
            let start = offset + relative;
            let final_byte = bytes[start + 2..]
                .iter()
                .position(|byte| (0x40..=0x7e).contains(byte))
                .map(|index| bytes[start + 2 + index]);
            let Some(final_byte) = final_byte else {
                break;
            };
            assert!(!matches!(final_byte, b'A'..=b'H' | b'f'));
            offset = start + 3;
        }
    }

    #[test]
    fn serialize_rejects_empty_payload_and_invalid_geometry() {
        let id = NonZeroU32::new(1).unwrap();
        assert_eq!(
            serialize(b"", id, 1, 1, false),
            Err(SerializeError::EmptyPayload)
        );
        for (cols, rows) in [(0, 1), (1, 0), (257, 1), (1, 257)] {
            assert_eq!(
                serialize(b"x", id, cols, rows, false),
                Err(SerializeError::InvalidGeometry)
            );
        }
    }

    #[test]
    fn base64_matches_rfc4648_vectors_and_round_trips() {
        let vectors: &[(&[u8], &str)] = &[
            (&b""[..], ""),
            (&b"f"[..], "Zg=="),
            (&b"fo"[..], "Zm8="),
            (&b"foo"[..], "Zm9v"),
            (&b"foob"[..], "Zm9vYg=="),
            (&b"fooba"[..], "Zm9vYmE="),
            (&b"foobar"[..], "Zm9vYmFy"),
        ];
        for (input, expected) in vectors {
            let encoded = base64(input).unwrap();
            assert_eq!(encoded, expected.as_bytes());
            assert_eq!(decode_base64(&encoded), *input);
        }
    }

    #[test]
    fn direct_apc_stream_has_exact_framing_and_bounded_chunks() {
        let id = NonZeroU32::new(0x0102_0304).unwrap();
        for length in [1, 3071, 3072, 3073, 4095, 4096, 4097, 8193] {
            let original: Vec<u8> = (0..length).map(|byte| (byte % 251) as u8).collect();
            let serialized = serialize(&original, id, 17, 3, false).unwrap();
            let commands = parse_apc_stream(&serialized.transfer);
            assert!(!commands.is_empty());

            let first_fields = control_fields(&commands[0].control);
            assert_eq!(first_fields.len(), 10);
            assert_eq!(first_fields[0], b"a=T");
            assert_eq!(first_fields[1], b"t=d");
            assert_eq!(first_fields[2], b"f=100");
            assert_eq!(first_fields[3], b"U=1");
            assert_eq!(first_fields[4], format!("i={}", id).as_bytes());
            assert_eq!(first_fields[5], b"c=17");
            assert_eq!(first_fields[6], b"r=3");
            assert_eq!(first_fields[7], b"q=2");
            assert_eq!(first_fields[8], b"N=1");
            assert_eq!(
                first_fields[9],
                format!("m={}", usize::from(commands.len() > 1)).as_bytes()
            );

            let mut decoded = Vec::new();
            for (index, command) in commands.iter().enumerate() {
                assert!(command.payload.len() <= MAX_CHUNK);
                if index + 1 < commands.len() {
                    assert_eq!(command.payload.len() % 4, 0);
                }
                let fields = control_fields(&command.control);
                if index == 0 {
                    assert_eq!(fields, first_fields);
                } else {
                    assert_eq!(
                        fields,
                        vec![
                            format!("m={}", usize::from(index + 1 < commands.len())).as_bytes(),
                            b"q=2",
                        ]
                    );
                }
                assert_eq!(
                    fields
                        .iter()
                        .filter(|field| field.starts_with(b"q="))
                        .count(),
                    1
                );
                assert_eq!(
                    fields
                        .iter()
                        .filter(|field| field.starts_with(b"m="))
                        .count(),
                    1
                );
                decoded.extend(decode_base64(&command.payload));
            }
            assert_eq!(decoded, original);

            for key in ["a=", "t=", "f=", "U=", "i=", "c=", "r=", "N="] {
                let count = commands
                    .iter()
                    .flat_map(|command| control_fields(&command.control))
                    .filter(|field| field.starts_with(key.as_bytes()))
                    .count();
                assert_eq!(count, 1, "metadata {key:?}");
            }
            assert!(control_fields(&commands.last().unwrap().control).contains(&b"m=0".as_slice()));

            let rebuilt: Vec<u8> = commands.iter().flat_map(frame_apc).collect();
            assert_eq!(rebuilt, serialized.transfer);
        }
    }

    #[test]
    fn tmux_wraps_single_and_multiple_apcs_without_extra_escapes() {
        let id = NonZeroU32::new(0x0102_0304).unwrap();
        let single_direct = serialize(b"x", id, 1, 1, false).unwrap();
        let single_tmux = serialize(b"x", id, 1, 1, true).unwrap();
        let mut expected = b"\x1bPtmux;".to_vec();
        for byte in &single_direct.transfer {
            expected.push(*byte);
            if *byte == ESC {
                expected.push(ESC);
            }
        }
        expected.extend(b"\x1b\\");
        assert_eq!(single_tmux.transfer, expected);

        let original: Vec<u8> = (0..8193).map(|byte| (byte % 251) as u8).collect();
        let direct = serialize(&original, id, 17, 3, false).unwrap();
        let tmux = serialize(&original, id, 17, 3, true).unwrap();
        let direct_commands = parse_apc_stream(&direct.transfer);
        let wrapped_commands = parse_tmux_stream(&tmux.transfer);
        assert_eq!(wrapped_commands.len(), direct_commands.len());
        for (wrapped, direct) in wrapped_commands.iter().zip(direct_commands.iter()) {
            assert_eq!(wrapped, &frame_apc(direct));
        }
    }

    #[test]
    fn placeholders_use_ordered_diacritics_ids_and_exact_rows() {
        for (index, expected) in [
            (0, 0x0305),
            (1, 0x030D),
            (31, 0x0484),
            (63, 0x0658),
            (127, 0x082C),
            (191, 0x1DE1),
            (254, 0xA8E4),
            (255, 0xA8E5),
        ] {
            assert_eq!(DIACRITICS[index], expected);
        }
        assert_eq!(DIACRITICS.len(), 256);

        let low_id = NonZeroU32::new(0x0001_0203).unwrap();
        let high_id = NonZeroU32::new(0xff01_0203).unwrap();
        let max_id = NonZeroU32::new(0xffff_ffff).unwrap();
        for id in [low_id, high_id, max_id] {
            let serialized = serialize(b"x", id, 3, 3, false).unwrap();
            assert_exact_placeholder_rows(&serialized, id, 3, 3);
        }
        assert!(serialize(b"x", low_id, 257, 1, false).is_err());

        let all_diacritics = serialize(b"x", high_id, 256, 256, false).unwrap();
        assert_exact_placeholder_rows(&all_diacritics, high_id, 256, 256);
    }

    #[test]
    fn generated_image_ids_are_nonzero_and_vary() {
        let ids: Vec<NonZeroU32> = (0..32).map(|_| next_image_id()).collect();
        assert!(ids.iter().all(|id| id.get() != 0));
        let mut distinct = ids.iter().map(|id| id.get()).collect::<Vec<_>>();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() >= 2);
    }

    #[test]
    fn serialized_output_contains_no_queries_or_cursor_control() {
        for tmux in [false, true] {
            let serialized = serialize(
                b"payload",
                NonZeroU32::new(0xffff_ffff).unwrap(),
                3,
                3,
                tmux,
            )
            .unwrap();
            assert_no_forbidden_terminal_sequences(&serialized.transfer);
            for row in &serialized.placeholder_rows {
                assert_no_forbidden_terminal_sequences(row.as_bytes());
            }
        }
    }
}

//! Private, length-prefixed worker protocol. Public output uses Report JSON.
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

pub const MAX_PACKET_BYTES: usize = 32 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"HPD\x01";

pub fn write_packet<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_PACKET_BYTES {
        return Err(io::Error::other("worker packet exceeds 32 MiB"));
    }
    writer.write_all(MAGIC)?;
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

pub fn read_packet<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::other("unsupported worker protocol version"));
    }
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_PACKET_BYTES {
        return Err(io::Error::other("worker packet exceeds 32 MiB"));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets_reject_truncation_and_oversized_headers() {
        let mut bytes = vec![];
        write_packet(&mut bytes, &vec!["é", "line\nline"]).unwrap();
        let result: Vec<String> = read_packet(&mut bytes.as_slice()).unwrap();
        assert_eq!(result, ["é", "line\nline"]);
        assert!(read_packet::<Vec<String>>(&mut &bytes[..bytes.len() - 1]).is_err());
        let mut oversized = MAGIC.to_vec();
        oversized.extend(u32::MAX.to_le_bytes());
        assert!(read_packet::<Vec<String>>(&mut oversized.as_slice()).is_err());
    }
}

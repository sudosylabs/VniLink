//! Cursor reads and socket codecs shared by invitations and announcements.
//!
//! Both formats are length-prefixed byte machines. The readers return `None`
//! on truncation; each format maps that to its own error, so a truncated
//! invitation stays `InvalidInvitation` and a truncated frame stays `Malformed`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub(crate) fn read_u8(bytes: &[u8], cursor: &mut usize) -> Option<u8> {
    let value = *bytes.get(*cursor)?;
    *cursor += 1;
    Some(value)
}

pub(crate) fn read_array<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Option<[u8; N]> {
    let end = cursor.checked_add(N)?;
    if end > bytes.len() {
        return None;
    }
    let mut array = [0u8; N];
    array.copy_from_slice(&bytes[*cursor..end]);
    *cursor = end;
    Some(array)
}

/// Family tag, octets, and big-endian port, matching both wire formats.
pub(crate) fn write_sockaddr(bytes: &mut Vec<u8>, addr: &SocketAddr) {
    match addr {
        SocketAddr::V4(addr) => {
            bytes.push(4);
            bytes.extend_from_slice(&addr.ip().octets());
            bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        SocketAddr::V6(addr) => {
            bytes.push(6);
            bytes.extend_from_slice(&addr.ip().octets());
            bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
    }
}

/// Inverse of [`write_sockaddr`]. `None` covers truncation and an unknown
/// family alike; formats with distinct errors for those keep their own
/// dispatch and use [`read_array`] directly.
pub(crate) fn read_sockaddr(bytes: &[u8], cursor: &mut usize) -> Option<SocketAddr> {
    let family = read_u8(bytes, cursor)?;
    match family {
        4 => {
            let octets = read_array::<4>(bytes, cursor)?;
            let port = u16::from_be_bytes(read_array::<2>(bytes, cursor)?);
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port))
        }
        6 => {
            let octets = read_array::<16>(bytes, cursor)?;
            let port = u16::from_be_bytes(read_array::<2>(bytes, cursor)?);
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        _ => None,
    }
}

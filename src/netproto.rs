//! Protocol code with no hardware in it: IPv4, UDP, TCP, DHCP, DNS and the
//! HTTP parts a client needs. net.rs moves the bytes. This file builds and
//! reads them. Nothing here touches the device, so the tests run on the
//! build machine with `cargo test` in the simulator crate.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

pub const TCP_FIN: u8 = 0x01;
pub const TCP_SYN: u8 = 0x02;
pub const TCP_RST: u8 = 0x04;
pub const TCP_PSH: u8 = 0x08;
pub const TCP_ACK: u8 = 0x10;

pub const DHCP_DISCOVER: u8 = 1;
pub const DHCP_OFFER: u8 = 2;
pub const DHCP_REQUEST: u8 = 3;
pub const DHCP_ACK: u8 = 5;

/// Internet checksum (RFC 1071). Over data that already holds its own
/// checksum field, a valid packet gives 0.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Checksum for UDP and TCP, which covers a pseudo header with the addresses.
pub fn transport_checksum(src: [u8; 4], dst: [u8; 4], proto: u8, seg: &[u8]) -> u16 {
    let mut buf = Vec::with_capacity(12 + seg.len());
    buf.extend_from_slice(&src);
    buf.extend_from_slice(&dst);
    buf.push(0);
    buf.push(proto);
    buf.extend_from_slice(&(seg.len() as u16).to_be_bytes());
    buf.extend_from_slice(seg);
    checksum(&buf)
}

fn be16(m: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*m.get(i)?, *m.get(i + 1)?]))
}

fn be32(m: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*m.get(i)?, *m.get(i + 1)?, *m.get(i + 2)?, *m.get(i + 3)?]))
}

fn ip_at(m: &[u8]) -> [u8; 4] {
    [m[0], m[1], m[2], m[3]]
}

// ---- IPv4 ----------------------------------------------------------------

pub struct Ipv4<'a> {
    pub proto: u8,
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub payload: &'a [u8],
}

pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4<'_>> {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = ((p[0] & 0x0F) as usize) * 4;
    let total = be16(p, 2)? as usize;
    if ihl < 20 || p.len() < ihl || total < ihl {
        return None;
    }
    let end = total.min(p.len());
    Some(Ipv4 {
        proto: p[9],
        src: ip_at(&p[12..16]),
        dst: ip_at(&p[16..20]),
        payload: &p[ihl..end],
    })
}

pub fn build_ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
    let total = (20 + payload.len()) as u16;
    let mut h = [0u8; 20];
    h[0] = 0x45;
    h[2..4].copy_from_slice(&total.to_be_bytes());
    h[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // don't fragment
    h[8] = 64;
    h[9] = proto;
    h[12..16].copy_from_slice(&src);
    h[16..20].copy_from_slice(&dst);
    let c = checksum(&h);
    h[10..12].copy_from_slice(&c.to_be_bytes());
    let mut out = Vec::with_capacity(20 + payload.len());
    out.extend_from_slice(&h);
    out.extend_from_slice(payload);
    out
}

// ---- UDP -----------------------------------------------------------------

pub struct Udp<'a> {
    pub sport: u16,
    pub dport: u16,
    pub data: &'a [u8],
}

pub fn build_udp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
    let mut s = Vec::with_capacity(8 + data.len());
    s.extend_from_slice(&sport.to_be_bytes());
    s.extend_from_slice(&dport.to_be_bytes());
    s.extend_from_slice(&((8 + data.len()) as u16).to_be_bytes());
    s.extend_from_slice(&[0, 0]);
    s.extend_from_slice(data);
    let mut c = transport_checksum(src, dst, PROTO_UDP, &s);
    if c == 0 {
        c = 0xFFFF; // zero means "no checksum", so send all ones instead
    }
    s[6..8].copy_from_slice(&c.to_be_bytes());
    s
}

pub fn parse_udp(src: [u8; 4], dst: [u8; 4], seg: &[u8]) -> Option<Udp<'_>> {
    let len = be16(seg, 4)? as usize;
    if len < 8 || len > seg.len() {
        return None;
    }
    let seg = &seg[..len];
    if be16(seg, 6)? != 0 && transport_checksum(src, dst, PROTO_UDP, seg) != 0 {
        return None;
    }
    Some(Udp { sport: be16(seg, 0)?, dport: be16(seg, 2)?, data: &seg[8..] })
}

// ---- TCP -----------------------------------------------------------------

pub struct Tcp<'a> {
    pub sport: u16,
    pub dport: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub data: &'a [u8],
}

pub fn build_tcp(
    src: [u8; 4],
    dst: [u8; 4],
    sport: u16,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    mss: Option<u16>,
    window: u16,
    data: &[u8],
) -> Vec<u8> {
    let hlen = if mss.is_some() { 24 } else { 20 };
    let mut s = vec![0u8; hlen];
    s[0..2].copy_from_slice(&sport.to_be_bytes());
    s[2..4].copy_from_slice(&dport.to_be_bytes());
    s[4..8].copy_from_slice(&seq.to_be_bytes());
    s[8..12].copy_from_slice(&ack.to_be_bytes());
    s[12] = ((hlen / 4) as u8) << 4;
    s[13] = flags;
    s[14..16].copy_from_slice(&window.to_be_bytes());
    if let Some(m) = mss {
        s[20] = 2; // option: maximum segment size
        s[21] = 4;
        s[22..24].copy_from_slice(&m.to_be_bytes());
    }
    s.extend_from_slice(data);
    let c = transport_checksum(src, dst, PROTO_TCP, &s);
    s[16..18].copy_from_slice(&c.to_be_bytes());
    s
}

pub fn parse_tcp(src: [u8; 4], dst: [u8; 4], seg: &[u8]) -> Option<Tcp<'_>> {
    if seg.len() < 20 {
        return None;
    }
    let doff = ((seg[12] >> 4) as usize) * 4;
    if doff < 20 || doff > seg.len() {
        return None;
    }
    if transport_checksum(src, dst, PROTO_TCP, seg) != 0 {
        return None;
    }
    Some(Tcp {
        sport: be16(seg, 0)?,
        dport: be16(seg, 2)?,
        seq: be32(seg, 4)?,
        ack: be32(seg, 8)?,
        flags: seg[13],
        data: &seg[doff..],
    })
}

// ---- DHCP ----------------------------------------------------------------

pub struct DhcpReply {
    pub msg: u8,
    pub yiaddr: [u8; 4],
    pub mask: [u8; 4],
    pub router: [u8; 4],
    pub dns: [u8; 4],
    pub server: [u8; 4],
}

/// A DHCP message (BOOTP wrapper plus options). `requested` and `server`
/// are used in the REQUEST that accepts an offer.
pub fn build_dhcp(
    mac: [u8; 6],
    xid: u32,
    msg: u8,
    requested: Option<[u8; 4]>,
    server: Option<[u8; 4]>,
) -> Vec<u8> {
    let mut p = vec![0u8; 240];
    p[0] = 1; // BOOTREQUEST
    p[1] = 1; // ethernet
    p[2] = 6; // hardware address length
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // reply by broadcast
    p[28..34].copy_from_slice(&mac);
    p[236..240].copy_from_slice(&[99, 130, 83, 99]); // magic cookie
    p.extend_from_slice(&[53, 1, msg]);
    if let Some(ip) = requested {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip);
    }
    if let Some(ip) = server {
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&ip);
    }
    p.extend_from_slice(&[55, 3, 1, 3, 6]); // ask for mask, router, dns
    p.push(255);
    while p.len() < 300 {
        p.push(0);
    }
    p
}

/// Read a server reply that answers our `xid` and was sent to our `mac`.
pub fn parse_dhcp(p: &[u8], xid: u32, mac: [u8; 6]) -> Option<DhcpReply> {
    if p.len() < 240 || p[0] != 2 || be32(p, 4)? != xid || p[28..34] != mac {
        return None;
    }
    if p[236..240] != [99, 130, 83, 99] {
        return None;
    }
    let mut r = DhcpReply {
        msg: 0,
        yiaddr: ip_at(&p[16..20]),
        mask: [255, 255, 255, 0],
        router: [0; 4],
        dns: [0; 4],
        server: [0; 4],
    };
    let mut i = 240;
    while i < p.len() {
        let code = p[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = *p.get(i + 1)? as usize;
        let v = p.get(i + 2..i + 2 + len)?;
        match code {
            53 if len >= 1 => r.msg = v[0],
            1 if len >= 4 => r.mask = ip_at(v),
            3 if len >= 4 => r.router = ip_at(v),
            6 if len >= 4 => r.dns = ip_at(v),
            54 if len >= 4 => r.server = ip_at(v),
            _ => {}
        }
        i += 2 + len;
    }
    if r.msg == 0 {
        None
    } else {
        Some(r)
    }
}

// ---- DNS -----------------------------------------------------------------

pub fn build_dns_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::new();
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00]); // recursion desired
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // one question
    for label in name.split('.').filter(|l| !l.is_empty()) {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0, 1, 0, 1]); // type A, class IN
    q
}

/// Skip a name, which is either labels ending in 0 or a compression pointer.
fn skip_name(m: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    loop {
        let len = *m.get(i)? as usize;
        if len == 0 {
            return Some(i + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(i + 2);
        }
        i += 1 + len;
    }
}

/// The first IPv4 address in a reply to query `id`, or None.
pub fn parse_dns_a(m: &[u8], id: u16) -> Option<[u8; 4]> {
    if be16(m, 0)? != id {
        return None;
    }
    let flags = be16(m, 2)?;
    if flags & 0x8000 == 0 || flags & 0x000F != 0 {
        return None; // not a response, or an error code
    }
    let questions = be16(m, 4)?;
    let answers = be16(m, 6)?;
    let mut i = 12;
    for _ in 0..questions {
        i = skip_name(m, i)? + 4;
    }
    for _ in 0..answers {
        i = skip_name(m, i)?;
        let rtype = be16(m, i)?;
        let rdlen = be16(m, i + 8)? as usize;
        let rdata = i + 10;
        if rtype == 1 && rdlen == 4 {
            return Some(ip_at(m.get(rdata..rdata + 4)?));
        }
        i = rdata + rdlen;
    }
    None
}

// ---- HTTP ----------------------------------------------------------------

/// Split "http://host:port/path" into its parts. Only plain HTTP is supported.
pub fn parse_url(url: &str) -> Result<(String, u16, String), &'static str> {
    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else if url.starts_with("https://") {
        return Err("https is not supported yet, only http://");
    } else {
        return Err("the address must start with http://");
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port number")?),
        None => (hostport, 80),
    };
    if host.is_empty() {
        return Err("the address has no host name");
    }
    Ok((String::from(host), port, String::from(path)))
}

pub fn build_http_get(host: &str, path: &str) -> Vec<u8> {
    format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: AnarchyOS\r\nConnection: close\r\n\r\n",
        path, host
    )
    .into_bytes()
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = find(b, b"\r\n")?;
        let line = core::str::from_utf8(&b[..line_end]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

/// Read a complete HTTP/1.x response. Returns None if the bytes end early.
pub fn parse_response(raw: &[u8]) -> Option<Response> {
    let end = find(raw, b"\r\n\r\n")?;
    let head = core::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok()?;

    let mut length = None;
    let mut chunked = false;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            if key.eq_ignore_ascii_case("content-length") {
                length = value.parse::<usize>().ok();
            } else if key.eq_ignore_ascii_case("transfer-encoding")
                && value.eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            }
        }
    }

    let rest = &raw[end + 4..];
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(n) = length {
        if rest.len() < n {
            return None;
        }
        rest[..n].to_vec()
    } else {
        rest.to_vec()
    };
    Some(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    const ME: [u8; 4] = [10, 0, 2, 15];
    const SERVER: [u8; 4] = [10, 0, 2, 2];

    #[test]
    fn checksum_matches_rfc1071_example() {
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data), 0x220d);
    }

    #[test]
    fn ipv4_round_trip() {
        let packet = build_ipv4(ME, SERVER, PROTO_UDP, b"abc");
        assert_eq!(checksum(&packet[..20]), 0, "header checksum must verify");
        let p = parse_ipv4(&packet).unwrap();
        assert_eq!((p.proto, p.src, p.dst), (PROTO_UDP, ME, SERVER));
        assert_eq!(p.payload, b"abc");
    }

    #[test]
    fn udp_round_trip_and_bad_checksum() {
        let seg = build_udp(ME, SERVER, 68, 67, b"hello");
        let u = parse_udp(ME, SERVER, &seg).unwrap();
        assert_eq!((u.sport, u.dport, u.data), (68, 67, &b"hello"[..]));
        let mut bad = seg.clone();
        bad[9] ^= 0xFF;
        assert!(parse_udp(ME, SERVER, &bad).is_none());
    }

    #[test]
    fn tcp_round_trip_with_mss_option() {
        let seg = build_tcp(ME, SERVER, 50000, 80, 1000, 2000, TCP_SYN | TCP_ACK, Some(1460), 16384, b"");
        let t = parse_tcp(ME, SERVER, &seg).unwrap();
        assert_eq!((t.sport, t.dport, t.seq, t.ack), (50000, 80, 1000, 2000));
        assert_eq!(t.flags, TCP_SYN | TCP_ACK);
        assert!(t.data.is_empty());

        let data = build_tcp(ME, SERVER, 50000, 80, 1, 1, TCP_PSH | TCP_ACK, None, 16384, b"GET /");
        let t = parse_tcp(ME, SERVER, &data).unwrap();
        assert_eq!(t.data, b"GET /");
        assert!(
            parse_tcp([10, 0, 2, 99], SERVER, &data).is_none(),
            "a different source address must fail the checksum"
        );
    }

    #[test]
    fn dhcp_request_has_expected_options() {
        let p = build_dhcp(MAC, 0xABCD_1234, DHCP_DISCOVER, None, None);
        assert_eq!(p[0], 1);
        assert_eq!(&p[4..8], &0xABCD_1234u32.to_be_bytes());
        assert_eq!(&p[28..34], &MAC);
        assert_eq!(&p[240..243], &[53, 1, DHCP_DISCOVER]);
        assert!(p.len() >= 300);
    }

    #[test]
    fn dhcp_offer_is_parsed() {
        let mut r = vec![0u8; 240];
        r[0] = 2;
        r[4..8].copy_from_slice(&7u32.to_be_bytes());
        r[16..20].copy_from_slice(&[10, 0, 2, 15]);
        r[28..34].copy_from_slice(&MAC);
        r[236..240].copy_from_slice(&[99, 130, 83, 99]);
        r.extend_from_slice(&[53, 1, 2, 1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 2, 2]);
        r.extend_from_slice(&[6, 4, 10, 0, 2, 3, 54, 4, 10, 0, 2, 2, 255]);
        let d = parse_dhcp(&r, 7, MAC).unwrap();
        assert_eq!(d.msg, DHCP_OFFER);
        assert_eq!(d.yiaddr, ME);
        assert_eq!(d.mask, [255, 255, 255, 0]);
        assert_eq!(d.router, SERVER);
        assert_eq!(d.dns, [10, 0, 2, 3]);
        assert_eq!(d.server, SERVER);
        assert!(parse_dhcp(&r, 8, MAC).is_none(), "another transaction must be ignored");
    }

    #[test]
    fn dns_query_and_answer() {
        let q = build_dns_query(0x1234, "example.com");
        assert_eq!(&q[12..25], b"\x07example\x03com\0");
        // Build a response: same question, one answer that points back to the name.
        let mut r = q.clone();
        r[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        r[6..8].copy_from_slice(&1u16.to_be_bytes());
        r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
        assert_eq!(parse_dns_a(&r, 0x1234), Some([93, 184, 216, 34]));
        assert_eq!(parse_dns_a(&r, 0x9999), None);
    }

    #[test]
    fn url_parts() {
        let (h, p, path) = parse_url("http://example.com:8080/a/b.txt").unwrap();
        assert_eq!((h.as_str(), p, path.as_str()), ("example.com", 8080, "/a/b.txt"));
        let (h, p, path) = parse_url("http://example.com").unwrap();
        assert_eq!((h.as_str(), p, path.as_str()), ("example.com", 80, "/"));
        assert!(parse_url("https://example.com").is_err());
        assert!(parse_url("ftp://example.com").is_err());
    }

    #[test]
    fn http_response_with_length_and_chunks() {
        let raw = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let r = parse_response(raw).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let r = parse_response(chunked).unwrap();
        assert_eq!(r.body.as_slice(), b"abcde");

        let short = b"HTTP/1.0 200 OK\r\nContent-Length: 9\r\n\r\nhi";
        assert!(parse_response(short).is_none(), "a cut-short body must be reported");
    }
}

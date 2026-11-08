//! Parses one line of `ping` output.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PingLine {
    /// `64 bytes from …: icmp_seq=0 ttl=64 time=2.906 ms`
    Reply { ms: f64 },
    /// `Request timeout for icmp_seq 1`
    Timeout,
    /// Any other line, such as the header
    Other,
}

pub fn parse_line(line: &str) -> PingLine {
    if line.starts_with("Request timeout for icmp_seq") {
        return PingLine::Timeout;
    }
    let ms = line
        .split_once("time=")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|v| v.parse().ok());
    match ms {
        Some(ms) => PingLine::Reply { ms },
        None => PingLine::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_line() {
        assert_eq!(
            parse_line("64 bytes from 192.168.11.1: icmp_seq=0 ttl=64 time=2.906 ms"),
            PingLine::Reply { ms: 2.906 }
        );
    }

    #[test]
    fn timeout_line() {
        assert_eq!(parse_line("Request timeout for icmp_seq 1"), PingLine::Timeout);
    }

    #[test]
    fn header_line_is_other() {
        assert_eq!(parse_line("PING 1.1.1.1 (1.1.1.1): 56 data bytes"), PingLine::Other);
    }
}

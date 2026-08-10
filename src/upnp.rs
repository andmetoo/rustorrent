use std::net::UdpSocket;
use std::time::Duration;

use crate::http;

const SSDP_ADDR: &str = "239.255.255.250:1900";
const SSDP_TIMEOUT: Duration = Duration::from_secs(2);

pub fn map_port(port: u16) -> Result<(), String> {
    let location = discover_gateway().ok_or_else(|| "upnp gateway not found".to_string())?;
    let description = http::get_same_origin(&location, 512 * 1024)?;
    let control = parse_control_url(&description, &location)
        .ok_or_else(|| "upnp control url not found".to_string())?;

    for protocol in ["TCP", "UDP"] {
        let body = build_add_port_mapping(port, protocol, &control.service_type);
        let headers = vec![
            ("Content-Type", "text/xml; charset=\"utf-8\"".to_string()),
            (
                "SOAPAction",
                format!("\"{}#AddPortMapping\"", control.service_type),
            ),
        ];
        let _ = http::post(&control.url, &headers, body.as_bytes(), 128 * 1024)?;
    }
    Ok(())
}

fn discover_gateway() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.set_read_timeout(Some(SSDP_TIMEOUT));
    let msg = "\
M-SEARCH * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
MAN: \"ssdp:discover\"\r\n\
MX: 2\r\n\
ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
\r\n";
    let _ = socket.send_to(msg.as_bytes(), SSDP_ADDR);
    let mut buf = [0u8; 2048];
    if let Ok((n, source)) = socket.recv_from(&mut buf) {
        let text = String::from_utf8_lossy(&buf[..n]);
        for line in text.split("\r\n") {
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("location") {
                    let value = value.trim();
                    if (value.starts_with("http://") || value.starts_with("https://"))
                        && !value.bytes().any(|b| b.is_ascii_control())
                        && http::url_host_ip(value) == Some(source.ip())
                    {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct ControlEndpoint {
    url: String,
    service_type: String,
}

fn parse_control_url(xml: &[u8], base: &str) -> Option<ControlEndpoint> {
    fn local_name(tag: &str) -> &str {
        tag.rsplit(':').next().unwrap_or(tag)
    }

    fn find_service(node: &crate::xml::XmlNode) -> Option<(&str, &str)> {
        if local_name(&node.tag) == "service" {
            let service_type = node
                .children
                .iter()
                .find(|child| local_name(&child.tag) == "serviceType")?
                .text
                .trim();
            if service_type.contains(":WANIPConnection:")
                || service_type.contains(":WANPPPConnection:")
            {
                let control = node
                    .children
                    .iter()
                    .find(|child| local_name(&child.tag) == "controlURL")?
                    .text
                    .trim();
                return Some((service_type, control));
            }
        }
        node.children.iter().find_map(find_service)
    }

    let root = crate::xml::parse(xml)?;
    let (service_type, control) = find_service(&root)?;
    let url = http::resolve_url(base, control).ok()?;
    if !http::same_origin(&url, base) {
        return None;
    }
    Some(ControlEndpoint {
        url,
        service_type: service_type.to_string(),
    })
}

fn build_add_port_mapping(port: u16, protocol: &str, service_type: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body>\
<u:AddPortMapping xmlns:u=\"{service_type}\">\
<NewRemoteHost></NewRemoteHost>\
<NewExternalPort>{port}</NewExternalPort>\
<NewProtocol>{protocol}</NewProtocol>\
<NewInternalPort>{port}</NewInternalPort>\
<NewInternalClient>{}</NewInternalClient>\
<NewEnabled>1</NewEnabled>\
<NewPortMappingDescription>rustorrent</NewPortMappingDescription>\
<NewLeaseDuration>0</NewLeaseDuration>\
</u:AddPortMapping>\
</s:Body>\
</s:Envelope>",
        local_ip().unwrap_or_else(|| "0.0.0.0".to_string())
    )
}

fn local_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.connect("8.8.8.8:53");
    socket.local_addr().ok().map(|addr| addr.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_control_url_supports_relative_and_absolute_urls() {
        let relative = b"
<service>
  <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
  <controlURL>/upnp/control/WANIPConn1</controlURL>
</service>";
        assert_eq!(
            parse_control_url(relative, "http://router.local/rootDesc.xml"),
            Some(ControlEndpoint {
                url: "http://router.local/upnp/control/WANIPConn1".to_string(),
                service_type: "urn:schemas-upnp-org:service:WANIPConnection:1".to_string(),
            })
        );

        let absolute = b"
<service>
  <serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
  <controlURL>http://router.local/control</controlURL>
</service>";
        assert_eq!(
            parse_control_url(absolute, "http://router.local/rootDesc.xml"),
            Some(ControlEndpoint {
                url: "http://router.local/control".to_string(),
                service_type: "urn:schemas-upnp-org:service:WANPPPConnection:1".to_string(),
            })
        );
    }

    #[test]
    fn parse_control_url_returns_none_when_service_missing() {
        let xml = b"<root><serviceType>urn:schemas-upnp-org:service:Other:1</serviceType></root>";
        assert_eq!(parse_control_url(xml, "http://router.local"), None);
    }

    #[test]
    fn add_port_mapping_body_contains_requested_port() {
        let body = build_add_port_mapping(
            51413,
            "UDP",
            "urn:schemas-upnp-org:service:WANPPPConnection:1",
        );
        assert!(body.contains("<NewExternalPort>51413</NewExternalPort>"));
        assert!(body.contains("<NewInternalPort>51413</NewInternalPort>"));
        assert!(body.contains("AddPortMapping"));
        assert!(body.contains("<NewProtocol>UDP</NewProtocol>"));
        assert!(body.contains("WANPPPConnection:1"));
    }
}

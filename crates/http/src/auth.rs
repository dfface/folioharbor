use crate::{problem_response, routes::AppState};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::request::Parts,
    response::Response,
};
use folioharbor_application::{actor::Actor, error::AppError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Clone, Copy, Debug)]
pub struct AuthenticatedActor(pub Actor);
#[derive(Clone, Copy, Debug)]
pub struct MaybeActor(pub Option<Actor>);
#[derive(Clone, Debug)]
pub struct ClientIpPrefix(pub String);

impl FromRequestParts<AppState> for AuthenticatedActor {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Actor>()
            .copied()
            .map(Self)
            .ok_or_else(|| problem_response(&parts.extensions, &AppError::Unauthenticated))
    }
}
impl FromRequestParts<AppState> for MaybeActor {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.extensions.get::<Actor>().copied()))
    }
}

impl FromRequestParts<AppState> for ClientIpPrefix {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        let prefix = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(|| "unknown".to_owned(), |connect| ip_prefix(connect.0.ip()));
        Ok(Self(prefix))
    }
}

fn ip_prefix(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(address) => {
            let mut octets = address.octets();
            octets[3] = 0;
            format!("{}/24", Ipv4Addr::from(octets))
        }
        IpAddr::V6(address) => {
            let mut segments = address.segments();
            segments[4..].fill(0);
            format!("{}/64", Ipv6Addr::from(segments))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ip_prefix;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn masks_client_addresses_before_rate_limit_keying() {
        assert_eq!(
            ip_prefix(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 129))),
            "192.0.2.0/24"
        );
        assert_eq!(
            ip_prefix(IpAddr::V6(Ipv6Addr::new(
                0x2001, 0x0db8, 0xabcd, 0x0012, 0x3456, 0x789a, 0xbcde, 0xf012,
            ))),
            "2001:db8:abcd:12::/64"
        );
    }
}

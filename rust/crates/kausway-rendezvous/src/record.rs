use std::fmt;
use std::net::IpAddr;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use iroh::{EndpointAddr, TransportAddr};
use iroh_dns::pkarr::{SignedPacket, Timestamp};
use kausway_transport::invite::{InviteLink, LINK_VERSION};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{Error, Secrets};

const DOMAIN: &[u8] = b"kausway-rendezvous-v1";
const TXT_NAME: &str = "_kausway";
const MAX_ENVELOPE: usize = 8192;
pub const RECORD_TTL_SECS: u32 = 300;
pub const MAX_RECORD_AGE_SECS: u64 = 3600;
pub const CLOCK_SKEW_SECS: u64 = 300;

/// Encrypted recovery copy of the original invitation; contains no keys or readable token.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SealedInvite(Vec<u8>);

impl fmt::Debug for SealedInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealedInvite([REDACTED])")
    }
}

impl SealedInvite {
    pub fn seal(secrets: &Secrets, invite: &InviteLink, expires_at: u64) -> Result<Self, Error> {
        if invite.v != LINK_VERSION || invite.token.is_empty() || invite.token.len() > 256 {
            return Err(Error::InvalidRecord);
        }
        InviteLink::decode(&invite.encode()).map_err(|error| match error {
            kausway_transport::invite::InviteError::TooLong => Error::TooLarge,
            _ => Error::InvalidRecord,
        })?;
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&(invite.v, &invite.addr, &invite.token))
                .map_err(|_| Error::InvalidRecord)?,
        );
        if plaintext.len() > MAX_ENVELOPE - 49 {
            return Err(Error::TooLarge);
        }
        let mut envelope = vec![1];
        envelope.extend_from_slice(&expires_at.to_be_bytes());
        let mut nonce = [0u8; 24];
        rand::rng().fill_bytes(&mut nonce);
        let ad = [DOMAIN, &envelope].concat();
        envelope.extend_from_slice(&nonce);
        let cipher = XChaCha20Poly1305::new((&secrets.encryption_key).into());
        envelope.extend_from_slice(
            &cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &plaintext,
                        aad: &ad,
                    },
                )
                .map_err(|_| Error::InvalidRecord)?,
        );
        Ok(Self(envelope))
    }

    pub fn open(&self, secrets: &Secrets, now: u64) -> Result<InviteLink, Error> {
        if self.0.len() < 49 || self.0.len() > MAX_ENVELOPE || self.0[0] != 1 {
            return Err(Error::InvalidRecord);
        }
        let ad = [DOMAIN, &self.0[..9]].concat();
        let cipher = XChaCha20Poly1305::new((&secrets.encryption_key).into());
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&self.0[9..33]),
                    Payload {
                        msg: &self.0[33..],
                        aad: &ad,
                    },
                )
                .map_err(|_| Error::InvalidRecord)?,
        );
        if now >= self.expires_at()? {
            return Err(Error::Expired);
        }
        let (version, addr, token): (u32, EndpointAddr, String) =
            serde_json::from_slice(&plaintext).map_err(|_| Error::InvalidRecord)?;
        let invite = InviteLink {
            v: version,
            addr,
            token,
        };
        // Reuse the existing invitation validator before returning anything to pairing.
        InviteLink::decode(&invite.encode()).map_err(|_| Error::InvalidRecord)
    }

    pub fn expires_at(&self) -> Result<u64, Error> {
        let bytes = self.0.get(1..9).ok_or(Error::InvalidRecord)?;
        Ok(u64::from_be_bytes(
            bytes.try_into().map_err(|_| Error::InvalidRecord)?,
        ))
    }
}

pub fn build_packet(
    secrets: &Secrets,
    invite: &InviteLink,
    expires_at: u64,
    sequence: u64,
    now: u64,
) -> Result<SignedPacket, Error> {
    validate_time(sequence, now)?;
    if now >= expires_at {
        return Err(Error::Expired);
    }
    let envelope = SealedInvite::seal(secrets, invite, expires_at)?;
    let encoded = URL_SAFE_NO_PAD.encode(&envelope.0);
    let values = encoded
        .as_bytes()
        .chunks(240)
        .enumerate()
        .map(|(i, chunk)| {
            format!(
                "{i:02}:{}",
                std::str::from_utf8(chunk).expect("base64 ASCII")
            )
        })
        .collect::<Vec<_>>();
    let secret = secrets.signing_key();
    let template = SignedPacket::from_txt_strings(&secret, TXT_NAME, values, RECORD_TTL_SECS)
        .map_err(|_| Error::TooLarge)?;
    // iroh-dns constructs DNS and validates its size; sign at the durable BEP44 sequence.
    let dns = template.encoded_packet();
    let mut signable = format!("3:seqi{sequence}e1:v{}:", dns.len()).into_bytes();
    signable.extend_from_slice(dns);
    let signature = secret.sign(&signable).to_bytes();
    let packet = SignedPacket::from_parts_unchecked(
        secret.public().as_bytes(),
        &signature,
        Timestamp::from_micros(sequence),
        dns,
    )
    .map_err(|_| Error::InvalidRecord)?;
    SignedPacket::from_bytes(packet.as_bytes()).map_err(|_| Error::InvalidRecord)
}

/// Keep every relay, prioritize direct routes, and never remove the final direct-only route.
/// The caller must supply the actual effective endpoint address (which reflects relay policy).
pub fn fit_packet(
    secrets: &Secrets,
    invite: &InviteLink,
    expires_at: u64,
    sequence: u64,
    now: u64,
) -> Result<SignedPacket, Error> {
    match build_packet(secrets, invite, expires_at, sequence, now) {
        Ok(packet) if has_route(&invite.addr) => return Ok(packet),
        Err(Error::TooLarge) => {}
        Err(error) => return Err(error),
        _ => return Err(Error::TooLarge),
    }
    let mut trimmed = invite.clone();
    let mut ips = Vec::new();
    trimmed.addr.addrs.retain(|addr| match addr {
        TransportAddr::Ip(ip) => {
            ips.push(*ip);
            false
        }
        _ => true,
    });
    ips.retain(usable_ip);
    ips.sort_by_key(|addr| (ip_rank(addr.ip()), *addr));
    if !has_route(&trimmed.addr) {
        let first = ips.iter().position(usable_ip).ok_or(Error::TooLarge)?;
        trimmed
            .addr
            .addrs
            .insert(TransportAddr::Ip(ips.remove(first)));
    }
    let mut fitted = build_packet(secrets, &trimmed, expires_at, sequence, now)?;
    for ip in ips {
        let route = TransportAddr::Ip(ip);
        trimmed.addr.addrs.insert(route.clone());
        match build_packet(secrets, &trimmed, expires_at, sequence, now) {
            Ok(packet) => fitted = packet,
            Err(Error::TooLarge) => {
                trimmed.addr.addrs.remove(&route);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(fitted)
}

fn has_route(addr: &EndpointAddr) -> bool {
    addr.addrs.iter().any(|a| match a {
        TransportAddr::Relay(_) => true,
        TransportAddr::Ip(ip) => usable_ip(ip),
        _ => false,
    })
}

fn usable_ip(ip: &std::net::SocketAddr) -> bool {
    !ip.ip().is_unspecified()
        && !ip.ip().is_multicast()
        && ip.port() != 0
        && !matches!(ip.ip(), IpAddr::V4(v4) if v4.is_broadcast())
        && !matches!(ip, std::net::SocketAddr::V6(v6) if v6.ip().is_unicast_link_local() && v6.scope_id() == 0)
}

fn ip_rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(ip) if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() => 3,
        IpAddr::V4(ip)
            if ip.is_private() || ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64 =>
        {
            2
        }
        IpAddr::V4(_) => 0,
        IpAddr::V6(ip) if ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unspecified() => {
            3
        }
        IpAddr::V6(ip) if ip.is_unique_local() => 2,
        IpAddr::V6(_) => 1,
    }
}

fn validate_time(sequence: u64, now: u64) -> Result<(), Error> {
    if sequence > i64::MAX as u64 {
        return Err(Error::InvalidRecord);
    }
    let now_micros = now.saturating_mul(1_000_000);
    if sequence > now_micros.saturating_add(CLOCK_SKEW_SECS * 1_000_000)
        || now_micros.saturating_sub(sequence) > MAX_RECORD_AGE_SECS * 1_000_000
    {
        return Err(Error::Expired);
    }
    Ok(())
}

pub fn decode_packet(
    secrets: &Secrets,
    packet: &SignedPacket,
    now: u64,
) -> Result<InviteLink, Error> {
    let packet = SignedPacket::from_bytes(packet.as_bytes()).map_err(|_| Error::InvalidRecord)?;
    if packet.public_key() != secrets.public_key() {
        return Err(Error::InvalidRecord);
    }
    validate_time(packet.timestamp().as_micros(), now)?;
    let mut chunks = packet
        .txt_records(TXT_NAME)
        .into_iter()
        .map(|value| {
            let (index, data) = value.split_once(':').ok_or(Error::InvalidRecord)?;
            if index.len() != 2
                || !index.bytes().all(|b| b.is_ascii_digit())
                || data.is_empty()
                || data.len() > 240
            {
                return Err(Error::InvalidRecord);
            }
            Ok((
                index.parse::<usize>().map_err(|_| Error::InvalidRecord)?,
                data.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if chunks.is_empty() {
        return Err(Error::InvalidRecord);
    }
    chunks.sort_by_key(|(i, _)| *i);
    let mut encoded = String::new();
    for (expected, (actual, data)) in chunks.into_iter().enumerate() {
        if actual != expected {
            return Err(Error::InvalidRecord);
        }
        encoded.push_str(&data);
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| Error::InvalidRecord)?;
    let invite = SealedInvite(envelope).open(secrets, now)?;
    if !has_route(&invite.addr) {
        return Err(Error::InvalidRecord);
    }
    Ok(invite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Code;
    use iroh::SecretKey;

    fn invite() -> InviteLink {
        InviteLink::new(
            EndpointAddr::new(SecretKey::from_bytes(&[7; 32]).public())
                .with_ip_addr("192.0.2.10:4321".parse().unwrap()),
            kausway_transport::pairing::generate_token(),
        )
    }

    #[test]
    fn real_invite_round_trip_is_confidential_and_uses_existing_pairing() {
        let invite = invite();
        let secrets = Code::generate().secrets();
        let packet = build_packet(&secrets, &invite, 1000, 100_000_000, 100).unwrap();
        assert!(packet.encoded_packet().len() <= 1000);
        assert!(
            !packet
                .as_bytes()
                .windows(invite.token.len())
                .any(|b| b == invite.token.as_bytes())
        );
        let recovered = decode_packet(&secrets, &packet, 100).unwrap();
        assert_eq!(recovered.encode(), invite.encode());
        let mut invitations = [kausway_transport::pairing::Invitation::new(
            "inv-test".into(),
            "Phone".into(),
            &invite.token,
            100,
            900,
        )];
        let mut limiter = kausway_transport::pairing::FailureLimiter::default();
        assert!(
            kausway_transport::pairing::redeem(
                &mut invitations,
                &mut limiter,
                &recovered.token,
                "device",
                100
            )
            .is_ok()
        );
        assert!(
            kausway_transport::pairing::redeem(
                &mut invitations,
                &mut limiter,
                &invite.token,
                "another",
                100
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_wrong_key_tampering_expiry_stale_and_future() {
        let secrets = Code::generate().secrets();
        let invite = invite();
        let packet = build_packet(&secrets, &invite, 1000, 100_000_000, 100).unwrap();
        assert!(decode_packet(&Code::generate().secrets(), &packet, 100).is_err());
        assert_eq!(decode_packet(&secrets, &packet, 1000), Err(Error::Expired));
        let mut bad = packet.as_bytes().to_vec();
        *bad.last_mut().unwrap() ^= 1;
        assert!(SignedPacket::from_bytes(&bad).is_err());
        let mut sealed = SealedInvite::seal(&secrets, &invite, 1000).unwrap();
        sealed.0[8] ^= 1;
        assert_eq!(sealed.open(&secrets, 100), Err(Error::InvalidRecord));
        assert!(validate_time(100_000_000, 3701).is_err());
        assert!(validate_time(401_000_000, 100).is_err());
    }

    #[test]
    fn fits_many_interfaces_without_breaking_direct_only_routes_or_original_url() {
        let secrets = Code::generate().secrets();
        for with_relay in [false, true] {
            let mut invite = invite();
            if with_relay {
                invite.addr.addrs.insert(TransportAddr::Relay(
                    "https://relay.example.com".parse().unwrap(),
                ));
            }
            for i in 0..80u16 {
                invite.addr.addrs.insert(TransportAddr::Ip(
                    format!("[fd00::{i:x}]:4321").parse().unwrap(),
                ));
            }
            let original = invite.encode();
            assert!(build_packet(&secrets, &invite, 1000, 100_000_000, 100).is_err());
            let packet = fit_packet(&secrets, &invite, 1000, 100_000_000, 100).unwrap();
            let resolved = decode_packet(&secrets, &packet, 100).unwrap();
            assert_eq!(resolved.addr.id, invite.addr.id);
            assert_eq!(resolved.token, invite.token);
            assert!(
                resolved
                    .addr
                    .addrs
                    .contains(&TransportAddr::Ip("192.0.2.10:4321".parse().unwrap()))
            );
            assert_eq!(invite.encode(), original);
            if with_relay {
                assert!(
                    resolved
                        .addr
                        .addrs
                        .iter()
                        .any(|a| matches!(a, TransportAddr::Relay(_)))
                );
            }
        }
    }

    #[test]
    fn unfit_minimum_record_fails_without_modifying_invite() {
        let secrets = Code::generate().secrets();
        let mut invite = invite();
        invite.addr.addrs.insert(TransportAddr::Relay(
            format!("https://relay.example.com/{}", "a".repeat(1500))
                .parse()
                .unwrap(),
        ));
        let original = invite.encode();
        assert_eq!(
            fit_packet(&secrets, &invite, 1000, 100_000_000, 100).unwrap_err(),
            Error::TooLarge
        );
        assert_eq!(InviteLink::decode(&original).unwrap(), invite);
    }

    #[test]
    fn chunk_order_is_independent_but_duplicates_gaps_and_malformed_records_fail() {
        let secrets = Code::generate().secrets();
        let invite = invite();
        let now = Timestamp::now().as_micros() / 1_000_000;
        let packet = build_packet(&secrets, &invite, now + 900, now * 1_000_000, now).unwrap();
        let chunks = packet.txt_records(TXT_NAME);
        assert!(chunks.len() >= 2);
        let sign = |chunks: Vec<String>| {
            SignedPacket::from_txt_strings(
                &secrets.signing_key(),
                TXT_NAME,
                chunks,
                RECORD_TTL_SECS,
            )
            .unwrap()
        };
        let mut reversed = chunks.clone();
        reversed.reverse();
        assert_eq!(
            decode_packet(&secrets, &sign(reversed), now).unwrap(),
            invite
        );
        let mut duplicate = chunks.clone();
        duplicate[1] = duplicate[0].clone();
        assert_eq!(
            decode_packet(&secrets, &sign(duplicate), now),
            Err(Error::InvalidRecord)
        );
        let mut gap = chunks.clone();
        gap.remove(0);
        assert_eq!(
            decode_packet(&secrets, &sign(gap), now),
            Err(Error::InvalidRecord)
        );
        for invalid in [
            vec![],
            vec!["0:abc".into()],
            vec!["00:".into()],
            vec!["00:!!!".into()],
        ] {
            assert_eq!(
                decode_packet(&secrets, &sign(invalid), now),
                Err(Error::InvalidRecord)
            );
        }
    }

    #[test]
    fn freshness_and_expiry_boundaries_are_exact() {
        assert!(validate_time(100_000_000, 3700).is_ok());
        assert_eq!(validate_time(100_000_000, 3701), Err(Error::Expired));
        assert!(validate_time(400_000_000, 100).is_ok());
        assert_eq!(validate_time(400_000_001, 100), Err(Error::Expired));
        assert_eq!(
            validate_time(i64::MAX as u64 + 1, 100),
            Err(Error::InvalidRecord)
        );
        let secrets = Code::generate().secrets();
        let packet = build_packet(&secrets, &invite(), 1000, 100_000_000, 100).unwrap();
        assert!(decode_packet(&secrets, &packet, 999).is_ok());
        assert_eq!(decode_packet(&secrets, &packet, 1000), Err(Error::Expired));
    }

    #[test]
    fn invalid_routes_do_not_become_successful_publications() {
        let secrets = Code::generate().secrets();
        for socket in [
            "0.0.0.0:42",
            "224.0.0.1:42",
            "255.255.255.255:42",
            "127.0.0.1:0",
            "[fe80::1]:42",
            "[ff02::1]:42",
        ] {
            let mut invite = invite();
            invite.addr.addrs.clear();
            invite
                .addr
                .addrs
                .insert(TransportAddr::Ip(socket.parse().unwrap()));
            assert_eq!(
                fit_packet(&secrets, &invite, 1000, 100_000_000, 100).unwrap_err(),
                Error::TooLarge
            );
        }
    }

    #[test]
    fn recovery_seal_cannot_accept_an_invite_that_cannot_reopen() {
        let secrets = Code::generate().secrets();
        let mut invite = invite();
        // Compact JSON fits the local envelope limit but the URL's Base64 expansion does not.
        invite.addr.addrs.insert(TransportAddr::Relay(
            format!("https://relay.example.com/{}", "a".repeat(6500))
                .parse()
                .unwrap(),
        ));
        assert!(InviteLink::decode(&invite.encode()).is_err());
        assert!(SealedInvite::seal(&secrets, &invite, 1000).is_err());
    }
}

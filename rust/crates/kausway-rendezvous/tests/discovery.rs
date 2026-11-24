//! Feasibility proof: independent processes' equivalent clients recover the existing invitation.
use std::{
    net::Ipv4Addr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{EndpointAddr, SecretKey};
use kausway_rendezvous::{
    Code,
    backend::{Network, NetworkConfig},
    decode_packet, fit_packet,
};
use kausway_transport::{
    invite::InviteLink,
    pairing::{FailureLimiter, Invitation, redeem},
};

#[tokio::test]
async fn encrypted_real_invitation_crosses_dht_and_enters_existing_redemption() {
    let testnet = mainline::Testnet::builder(3)
        .bind_address(Ipv4Addr::LOCALHOST)
        .build()
        .unwrap();
    let config = NetworkConfig {
        bootstrap: Some(
            testnet
                .bootstrap
                .iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        ),
        timeout: Duration::from_secs(10),
        ..Default::default()
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = kausway_transport::pairing::generate_token();
    let original = InviteLink::new(
        EndpointAddr::new(SecretKey::generate().public())
            .with_ip_addr("192.0.2.10:4321".parse().unwrap()),
        token.clone(),
    );
    let code = Code::generate();
    let packet = fit_packet(&code.secrets(), &original, now + 900, now * 1_000_000, now).unwrap();
    let publisher = Network::new(config.clone()).unwrap();
    publisher.publish(&packet).await.unwrap();
    drop(publisher);

    // Only the human-readable code crosses from publisher to independent resolver.
    let entered = Code::parse(&code.expose().to_ascii_lowercase()).unwrap();
    let secrets = entered.secrets();
    let key = secrets.public_key();
    let resolver = Network::new(config).unwrap();
    let recovered = resolver
        .resolve_validated(&key, move |candidate| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            decode_packet(&secrets, candidate, now)
        })
        .await
        .unwrap();
    assert_eq!(recovered.encode(), original.encode());
    let mut invitations = [Invitation::new(
        "inv-prototype".into(),
        "Phone".into(),
        &token,
        now,
        900,
    )];
    let mut limiter = FailureLimiter::default();
    assert!(
        redeem(
            &mut invitations,
            &mut limiter,
            &recovered.token,
            "phone",
            now
        )
        .is_ok()
    );
    assert!(
        redeem(
            &mut invitations,
            &mut limiter,
            &original.token,
            "another-phone",
            now
        )
        .is_err()
    );
}

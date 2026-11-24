//! Recovered invitations enter the existing Iroh pairing/HTTP path without client changes.
use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use iroh::{Endpoint, EndpointAddr, SecretKey};
use kausway_host::{
    registry::StateRegistry,
    state::{HostState, STATE_FILE, now_secs},
};
use kausway_rendezvous::{Code, decode_packet, fit_packet};
use kausway_transport::{
    client::ClientSession,
    host::{HostService, serve},
    invite::InviteLink,
    net::{EndpointOptions, RelayChoice, bind_endpoint},
    protocol::{ALPN, AuthOutcome},
    proxy::Origin,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

async fn endpoint(alpns: Vec<Vec<u8>>) -> Endpoint {
    bind_endpoint(
        SecretKey::generate(),
        alpns,
        EndpointOptions {
            relay: Some(RelayChoice::Disabled),
            relay_transport_only: false,
        },
    )
    .await
    .unwrap()
}

async fn address(endpoint: &Endpoint) -> EndpointAddr {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let addr = endpoint.addr();
            if addr.addrs.iter().any(|a| a.is_ip()) {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("local endpoint address")
}

#[tokio::test(flavor = "multi_thread")]
async fn recovered_url_pairs_serves_http_and_reconnects_without_rendezvous() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let directory = std::env::temp_dir().join(format!(
            "kausway-rendezvous-e2e-{}-{}",
            std::process::id(),
            Code::generate().secrets().public_key()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let state = Arc::new(HostState::open(&directory.join(STATE_FILE), None).unwrap());
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_task = tokio::spawn(async move {
            loop {
                let (socket, _) = origin.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(socket),
                            service_fn(|_| async {
                                Ok::<_, std::convert::Infallible>(Response::new(Full::new(
                                    Bytes::from_static(b"rendezvous pairing works"),
                                )))
                            }),
                        )
                        .await;
                });
            }
        });
        let host = endpoint(vec![ALPN.to_vec()]).await;
        let host_addr = address(&host).await;
        let now = now_secs();
        let (invitation, token, code) = state
            .create_rendezvous_invitation("Integration", None, now, host_addr.clone())
            .unwrap();
        let original = InviteLink::new(host_addr.clone(), token);
        // Separate code parsing/key derivation simulates a different application's resolver.
        let packet = fit_packet(
            &code.secrets(),
            &original,
            invitation.expires_at,
            now * 1_000_000,
            now,
        )
        .unwrap();
        let resolver = Code::parse(&code.expose()).unwrap();
        let recovered = decode_packet(&resolver.secrets(), &packet, now).unwrap();
        assert_eq!(recovered.encode(), original.encode());

        let shutdown = CancellationToken::new();
        let service = HostService::new(
            Origin::parse(&format!("http://{origin_addr}")).unwrap(),
            Arc::new(StateRegistry::new(state.clone())),
        );
        let host_task = tokio::spawn(serve(host, service, shutdown.clone()));
        let client = endpoint(vec![]).await;
        let session =
            ClientSession::connect(&client, recovered.addr, "Phone", Some(&recovered.token))
                .await
                .unwrap();
        assert_eq!(session.outcome(), AuthOutcome::Paired);
        let mut response = session.request("GET", "/", vec![], None).await.unwrap();
        assert_eq!(response.head.status, 200);
        let body = response.body.read_to_end(1024).await.unwrap();
        assert_eq!(body, b"rendezvous pairing works");
        session.close().await;

        // No code lookup, record or token is needed on reconnect.
        let reconnect = ClientSession::connect(&client, host_addr, "Phone", None)
            .await
            .unwrap();
        assert_eq!(reconnect.outcome(), AuthOutcome::Authorized);
        reconnect.close().await;
        client.close().await;
        shutdown.cancel();
        host_task.await.unwrap();
        origin_task.abort();
        std::fs::remove_dir_all(directory).unwrap();
    })
    .await
    .expect("pairing test deadline");
}

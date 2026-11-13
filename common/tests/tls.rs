//! Certificates: the client reads the server's fingerprint, and a changed one is refused.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use common::tls::{acceptor, check_pin, connect_blocking, fingerprint, new_identity};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;

#[test]
fn client_reads_server_fingerprint_and_pins_it() {
    let (cert_pem, key_pem) = new_identity().unwrap();
    let expected = fingerprint(&CertificateDer::from_pem_slice(cert_pem.as_bytes()).unwrap());
    let acceptor = acceptor(&cert_pem, &key_pem).unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    runtime.spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else { break };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(socket).await {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let mut lines = BufReader::new(&mut tls).lines();
                    if let Ok(Some(_)) = lines.next_line().await {
                        let _ = lines.get_mut().get_mut().write_all(b"{\"type\":\"error\",\"text\":\"hi\"}\n").await;
                    }
                }
            });
        }
    });

    let (mut stream, seen) = connect_blocking(&addr, Duration::from_secs(5)).unwrap();
    assert_eq!(seen, expected, "client should see the server's own certificate");
    stream.write_all(b"ping\n").unwrap();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).unwrap();
    assert!(reply.contains("hi"));

    // First time: remember it. Same again: fine. A different certificate: refused.
    assert_eq!(check_pin(None, &seen), Ok(true));
    assert_eq!(check_pin(Some(&seen), &seen), Ok(false));
    assert!(check_pin(Some(&seen), "someone-else").is_err());
}

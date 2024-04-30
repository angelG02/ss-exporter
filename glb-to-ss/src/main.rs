use std::{io::BufReader, net::ToSocketAddrs, sync::Arc};

use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{self, pki_types},
    TlsConnector,
};
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Trace initialization
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Could not set default trace subscriver!");

    let mut args = std::env::args();
    args.next();

    let Some(server) = args.next() else {
        error!(
            "Please provide the full path of a model you would like to send to the Asset Server!"
        );
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };

    let Some(model_path) = args.next() else {
        error!(
            "Please provide the full path of a model you would like to send to the Asset Server!"
        );
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };

    let Some(auth_token) = args.next() else {
        error!("Please provide an auth token so that the server knows who you are!");
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };

    let domain_name = "as-http.angel-sunset.app";

    // Please don't flood or DoS me xd
    let addr = (server.as_str(), 8080)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;

    let mut root_cert_pem =
        BufReader::new(include_bytes!("certs/origin_ca_rsa_root.pem").as_slice());

    let mut root_cert_store = rustls::RootCertStore::empty();

    for cert in rustls_pemfile::certs(&mut root_cert_pem) {
        root_cert_store.add(cert?).unwrap();
    }

    let config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(&addr).await?;

    let domain = pki_types::ServerName::try_from(domain_name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?
        .to_owned();

    let mut stream = connector.connect(domain, stream).await?;

    let request = format!("put model {} {}", auth_token, "stub");

    stream.write_all(&request.len().to_ne_bytes()).await?;
    stream.write_all(&request.as_bytes()).await?;

    let mut response_size: [u8; 8] = [0; 8];
    stream.read_exact(&mut response_size).await?;
    let response_size = usize::from_ne_bytes(response_size);

    let mut response = vec![0u8; response_size];
    stream.read_exact(&mut response).await?;

    let response = std::str::from_utf8(&response).unwrap_or("Error Parsing Response");

    match response {
        "OK" => {
            let Ok(model_bytes) = std::fs::read(model_path.clone()) else {
                error!("File <{}> not found!", model_path);
                return Err(io::Error::from(io::ErrorKind::NotFound));
            };

            stream.write_all(&model_bytes.len().to_ne_bytes()).await?;
            stream.write_all(&model_bytes).await?;

            info!("Model Successfully sent!");
        }
        _ => {
            error!("Request denied! Response from server: {response}")
        }
    }

    Ok(())
}

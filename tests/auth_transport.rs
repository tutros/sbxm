//! Issue 330, decision 271: the host's token endpoint, `ureq`, against a local test server on
//! `127.0.0.1` (never Google: the real call is in the ignored real-`sbx` test).

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use common::Env;
use common::login::{NOW, write_login};
use sbxm::auth::{self, HostEndpoint, TokenEndpoint};

/// One request the server got: its request line, its headers (lowercased names) and its body.
struct Request {
    line: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// A server on `127.0.0.1` that answers one request with `status` and `body`, or never answers
/// with `None`. Returns its `/token` URL and where the request it got arrives.
fn serve(answer: Option<(u16, &'static str)>) -> (String, mpsc::Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut headers = Vec::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
        }
        let length = headers
            .iter()
            .find(|(n, _)| n == "content-length")
            .map_or(0, |(_, v)| v.parse::<usize>().unwrap());
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let _ = tx.send(Request {
            line: line.trim_end().to_owned(),
            headers,
            body: String::from_utf8(body).unwrap(),
        });
        match answer {
            Some((status, body)) => {
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            None => std::thread::sleep(Duration::from_secs(5)),
        }
    });
    (url, rx)
}

/// The host's endpoint, sent to `url` whatever URL it is asked for.
struct Local {
    url: String,
    endpoint: HostEndpoint,
}

impl Local {
    fn new(url: String) -> Self {
        Local {
            url,
            endpoint: HostEndpoint::GOOGLE,
        }
    }
}

impl TokenEndpoint for Local {
    fn check(&self) -> anyhow::Result<()> {
        self.endpoint.check()
    }

    fn post_form(&self, _url: &str, form: &[(&str, &str)]) -> anyhow::Result<(u16, String)> {
        self.endpoint.post_form(&self.url, form)
    }
}

const FORM: [(&str, &str); 2] = [("grant_type", "refresh_token"), ("refresh_token", "r t&=")];

#[test]
fn the_host_endpoint_waits_10_s_to_connect_and_30_s_in_all() {
    assert_eq!(HostEndpoint::GOOGLE.connect, Duration::from_secs(10));
    assert_eq!(HostEndpoint::GOOGLE.overall, Duration::from_secs(30));
}

#[test]
fn the_host_endpoint_can_be_used() {
    HostEndpoint::GOOGLE.check().unwrap();
}

#[test]
fn the_host_endpoint_posts_the_form_and_returns_the_status_and_the_body() {
    let (url, got) = serve(Some((200, r#"{"access_token":"t","expires_in":3599}"#)));

    let (status, body) = HostEndpoint::GOOGLE.post_form(&url, &FORM).unwrap();

    assert_eq!(status, 200);
    assert_eq!(body, r#"{"access_token":"t","expires_in":3599}"#);
    let request = got.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(request.line.starts_with("POST /token "), "{}", request.line);
    assert!(
        request.headers.iter().any(|(n, v)| n == "content-type"
            && v.starts_with("application/x-www-form-urlencoded")),
        "{:?}",
        request.headers
    );
    assert_eq!(
        request.body,
        "grant_type=refresh_token&refresh_token=r+t%26%3D"
    );
}

#[test]
fn an_answer_that_isnt_200_is_its_status_not_an_error() {
    let (url, _got) = serve(Some((401, r#"{"error":"invalid_grant"}"#)));
    let (status, _) = HostEndpoint::GOOGLE.post_form(&url, &FORM).unwrap();
    assert_eq!(status, 401);
}

#[test]
fn a_refresh_through_the_host_endpoint_caches_the_token() {
    let env = Env::new();
    write_login(&env.config_dir());
    let (url, got) = serve(Some((200, r#"{"access_token":"fresh","expires_in":3599}"#)));

    let token = auth::refresh(&env.config_dir(), NOW, &Local::new(url)).unwrap();

    assert_eq!(token, "fresh");
    let body = got.recv_timeout(Duration::from_secs(5)).unwrap().body;
    assert!(body.contains("refresh_token=fake-refresh"), "{body}");
    assert!(body.contains("client_secret=fake-secret"), "{body}");
}

#[test]
fn a_refused_refresh_names_the_sign_in_never_the_body_or_the_login() {
    for status in [400, 401] {
        let env = Env::new();
        write_login(&env.config_dir());
        let (url, _got) = serve(Some((status, r#"{"error":"invalid_grant-BODY"}"#)));

        let err = auth::refresh(&env.config_dir(), NOW, &Local::new(url)).unwrap_err();

        let err = format!("{err:#}");
        assert!(err.contains("sbxm auth antigravity"), "{err}");
        assert!(err.contains(&format!("HTTP {status}")), "{err}");
        for secret in ["BODY", "fake-refresh", "fake-secret"] {
            assert!(!err.contains(secret), "{err}");
        }
    }
}

#[test]
fn an_endpoint_that_cant_be_reached_names_the_sign_in_never_the_login() {
    let env = Env::new();
    write_login(&env.config_dir());
    // A port that was just free: nothing listens on it.
    let url = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/token", listener.local_addr().unwrap())
    };

    let err = auth::refresh(&env.config_dir(), NOW, &Local::new(url)).unwrap_err();

    let err = format!("{err:#}");
    assert!(err.contains("sbxm auth antigravity"), "{err}");
    for secret in ["fake-refresh", "fake-secret"] {
        assert!(!err.contains(secret), "{err}");
    }
}

#[test]
fn an_endpoint_that_never_answers_times_out_naming_the_sign_in() {
    let env = Env::new();
    write_login(&env.config_dir());
    let (url, _got) = serve(None);
    let local = Local {
        url,
        endpoint: HostEndpoint {
            connect: Duration::from_secs(10),
            overall: Duration::from_millis(300),
        },
    };

    let started = std::time::Instant::now();
    let err = auth::refresh(&env.config_dir(), NOW, &local).unwrap_err();

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    let err = format!("{err:#}");
    assert!(err.contains("sbxm auth antigravity"), "{err}");
    for secret in ["fake-refresh", "fake-secret"] {
        assert!(!err.contains(secret), "{err}");
    }
}

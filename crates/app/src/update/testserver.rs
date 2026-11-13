//! A one-thread HTTP/1.1 server on 127.0.0.1 for the update tests: real sockets, fixed routes.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub struct Route {
    pub path: &'static str,
    pub status: u16,
    pub body: Vec<u8>,
    /// None = body.len(); larger = short read.
    pub content_length: Option<usize>,
}

pub struct TestServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    /// Binds 127.0.0.1:0 and serves one connection at a time on a detached thread, closing each after
    /// one response. An unknown path gets a 404 with an empty body.
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::Builder::new()
            .name("vc-test-http".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    serve(stream, &routes, &log);
                }
            })
            .expect("spawn test server");
        Self { base, requests }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The request heads received so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}

fn serve(mut stream: TcpStream, routes: &[Route], log: &Mutex<Vec<String>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    log.lock().expect("requests lock").push(head.clone());
    let first = head.lines().next().unwrap_or_default();
    let route = routes
        .iter()
        .find(|r| first == format!("GET {} HTTP/1.1", r.path));
    let (status, body, length) = match route {
        Some(r) => (
            r.status,
            &r.body[..],
            r.content_length.unwrap_or(r.body.len()),
        ),
        None => (404, &[][..], 0),
    };
    let response =
        format!("HTTP/1.1 {status} X\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
    // The client may hang up early (a cancelled download); write errors are expected then.
    let _ = stream
        .write_all(response.as_bytes())
        .and_then(|()| stream.write_all(body));
    let _ = stream.shutdown(Shutdown::Write);
}

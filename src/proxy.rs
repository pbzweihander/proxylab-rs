#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate proxylab;
extern crate tokio;

use futures::compat::TokioDefaultSpawner;
use futures::task::SpawnExt;
use futures::{compat::*, prelude::*};
use proxylab::*;
use std::env::args;
use std::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::AsyncRead;

fn main() {
    let args = args().collect::<Vec<_>>();
    let port = args.get(1).and_then(|p| p.parse::<usize>().ok());

    if port.is_none() {
        eprintln!("usage: {} <port>\n", args[0]);
        return;
    }
    let port = port.unwrap();
    let addr = format!("0.0.0.0:{}", port).parse().unwrap();

    let listener = TcpListener::bind(&addr)
        .unwrap_or_else(|e| panic!("unable to bind TCP listener on {}: {:?}", addr, e));

    let server = async {
        let mut executor = TokioDefaultSpawner;
        let mut incomings = listener
            .incoming()
            .compat()
            .map_err(|e| eprintln!("accept failed: {:?}", e));

        while let Some(Ok(stream)) = await!(incomings.next()) {
            let handler = doit(stream);
            let _ = executor
                .spawn(handler)
                .map_err(|e| eprintln!("spawn failed: {:?}", e));
        }
    };
    let server = server.unit_error().boxed().compat();

    tokio::run(server);
}

async fn doit(stream: TcpStream) {
    let (reader, writer) = stream.split();
    let reader = BufReader::new(reader);
}

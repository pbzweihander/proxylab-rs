#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate proxylab;
extern crate tokio;

use futures::{compat::*, future::ready, prelude::*, stream::iter, task::SpawnExt};
use proxylab::*;
use std::{env::args, io::BufReader, iter::once, net::ToSocketAddrs};
use tokio::{
    io,
    net::{TcpListener, TcpStream},
    prelude::AsyncRead,
};

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
            let handler = doit(stream).unwrap_or_else(|e| eprintln!("io error: {:?}", e));
            let _ = executor
                .spawn(handler)
                .map_err(|e| eprintln!("spawn failed: {:?}", e));
        }
    };
    let server = server.unit_error().boxed().compat();

    tokio::run(server);
}

async fn doit(stream: TcpStream) -> Result<(), io::Error> {
    let (reader, writer) = stream.split();
    let reader = BufReader::new(reader);

    let req_fut = read_request(reader);
    let resp_fut = req_fut.and_then(|req| {
        println!("request: {} {} {}", req.method, req.uri.path, req.version);
        for header in req.headers.iter() {
            println!("{}", header);
        }
        println!();

        request_server(req)
    });
    let resp = await!(resp_fut);
    if let Err(e) = resp {
        return await!(client_error(writer, e));
    }
    let resp = resp.unwrap();

    await!(response(writer, resp))
}

async fn request_server(req: Request) -> Result<Response, HttpError> {
    let addrs = (req.uri.host.as_ref(), req.uri.port)
        .to_socket_addrs()
        .map_err(|e| HttpError::Error(format!("parsing socket addr failed: {:?}", e)))?;

    let stream = await!(iter(addrs)
        .then(|addr| TcpStream::connect(&addr).compat())
        .map(|r| r.map_err(|e| HttpError::Error(format!("connecting failed: {:?}", e))))
        .fold(
            Err(HttpError::Error("empty socket addrs".to_string())),
            |acc, s| ready(if acc.is_ok() { acc } else { s }),
        ))?;
    let (reader, writer) = stream.split();
    let reader = BufReader::new(reader);

    await!(request(writer, req))?;

    let resp = await!(read_response(reader))?;

    let headers: Vec<_> = resp
        .headers
        .iter()
        .filter(|h| {
            !h.starts_with("Content-Length:")
                && !(h.starts_with("Transfer-Encoding:") && h.contains("chunked"))
        })
        .map(|s| s.to_string())
        .chain(once(format!("Content-Length: {}", resp.content.len())))
        .collect();

    let resp = Response { headers, ..resp };

    println!("response: {} {} {}", resp.version, resp.status, resp.reason);
    for header in resp.headers.iter() {
        println!("{}", header);
    }
    println!("content {} bytes served", resp.content.len());

    Ok(resp)
}

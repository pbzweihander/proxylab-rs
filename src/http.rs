#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate proxylab;
extern crate tokio;

use futures::{compat::*, prelude::*, task::SpawnExt};
use proxylab::*;
use std::{env::args, io::BufReader};
use tokio::{
    fs, io,
    net::{TcpListener, TcpStream},
    prelude::{AsyncRead, AsyncWrite},
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

    let req = await!(read_request(reader));
    if let Err(e) = req {
        return await!(client_error(writer, e));
    }
    let req = req.unwrap();

    await!(req.log());

    if req.method != "GET" {
        return await!(client_error(writer, HttpError::NotImplemented(req.method)));
    }
    let filename = ".".to_string() + &req.uri.path;

    await!(serve_static(writer, filename))
}

async fn serve_static(writer: impl AsyncWrite, filename: String) -> Result<(), io::Error> {
    let file = await!(fs::File::open(filename.clone()).compat());
    if let Err(e) = file {
        match e.kind() {
            io::ErrorKind::NotFound => {
                return await!(client_error(writer, HttpError::NotFound(filename)));
            }
            io::ErrorKind::PermissionDenied => {
                return await!(client_error(writer, HttpError::Forbidden(filename)));
            }
            _ => return Err(e),
        }
    }
    let file = file.unwrap();
    let file_type = get_filetype(&filename);

    let (_, content) = await!(io::read_to_end(file, vec![]).compat())?;
    let size = content.len();

    let resp = Response {
        version: "HTTP/1.0".to_string(),
        status: 200,
        reason: "OK".to_string(),
        headers: vec![
            format!(
                "Content-Type: {}",
                match file_type {
                    FileType::Gif => "image/gif",
                    FileType::Html => "text/html",
                    FileType::Jpg => "image/jpg",
                    FileType::PlainText => "text/plain",
                    FileType::Png => "image/png",
                }
            ),
            format!("Content-Length: {}", size),
        ],
        content,
    };

    await!(response(writer, resp))?;

    println!("file {} size {} served\n", filename, size);

    Ok(())
}

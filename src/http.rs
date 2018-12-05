#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate regex;
extern crate tokio;
#[macro_use]
extern crate lazy_static;

use futures::compat::TokioDefaultSpawner;
use futures::future::ready;
use futures::task::SpawnExt;
use futures::{compat::*, prelude::*};
use regex::Regex;
use std::env::args;
use std::io::{BufRead, BufReader};
use tokio::fs;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::{AsyncRead, AsyncWrite};

enum HttpError {
    IsDirectory(String),
    Forbidden(String),
    NotFound(String),
    NotImplemented(String),
    Error(String),
}

enum FileType {
    Html,
    Jpg,
    Png,
    Gif,
    PlainText,
}

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

    let file_info = await!(read_request(reader));

    let _ = match file_info {
        Ok((filename, size)) => await!(serve_static(writer, filename, size)),
        Err(e) => await!(client_error(writer, e)),
    }
    .map_err(|e| eprintln!("io error: {:?}", e));
}

async fn read_request(reader: impl AsyncRead + BufRead) -> Result<(String, u64), HttpError> {
    let (reader, buf) = await!(io::read_until(reader, b'\n', vec![]).compat())
        .map_err(|e| HttpError::Error(format!("read failed: {:?}", e)))?;

    let line = String::from_utf8(buf)
        .map_err(|e| HttpError::Error(format!("decode request failed: {:?}", e)))?;
    let mut iter = line.split_whitespace();

    let method = iter.next();
    let uri = iter.next();
    let version = iter.next();

    let (method, uri, version) = method
        .and_then(|m| {
            uri.and_then(|u| version.map(|v| (m.to_string(), u.to_string(), v.to_string())))
        })
        .ok_or_else(|| HttpError::Error("request line parsing failed".to_string()))?;

    if method != "GET" {
        return Err(HttpError::NotImplemented(method.to_string()));
    }
    println!("request:\n{} {} {}", method, uri, version);
    await!(print_requesthdrs(reader));

    let filename =
        parse_uri(&uri).ok_or_else(|| HttpError::Error("uri parsing failed".to_string()))?;
    let filename1 = filename.clone();
    let metadata =
        await!(fs::metadata(filename.clone()).compat()).map_err(move |e| match e.kind() {
            io::ErrorKind::NotFound => HttpError::NotFound(filename),
            io::ErrorKind::PermissionDenied => HttpError::Forbidden(filename),
            _ => HttpError::Error(format!("file metadata error: {:?}", e)),
        })?;

    if metadata.is_dir() {
        return Err(HttpError::IsDirectory(filename1));
    }

    Ok((filename1, metadata.len()))
}

async fn serve_static(
    writer: impl AsyncWrite,
    filename: String,
    size: u64,
) -> Result<(), io::Error> {
    let file = await!(fs::File::open(filename.clone()).compat())?;
    let file_type = get_filetype(&filename);

    let line = "HTTP/1.0 200 OK\r\n".to_string();
    let header = format!(
        "Content-type: {}\r\nContent-Length: {}\r\n\r\n",
        match file_type {
            FileType::Gif => "image/gif",
            FileType::Html => "text/html",
            FileType::Jpg => "image/jpg",
            FileType::PlainText => "text/plain",
            FileType::Png => "image/png",
        },
        size
    );

    let (writer, _) = await!(io::write_all(writer, line).compat())?;
    let (writer, _) = await!(io::write_all(writer, header).compat())?;
    let _ = await!(io::copy(file, writer).compat())?;

    println!("file {} size {} served\n", filename, size);

    Ok(())
}

fn get_filetype(filename: &str) -> FileType {
    if filename.ends_with(".html") {
        FileType::Html
    } else if filename.ends_with(".jpg") {
        FileType::Jpg
    } else if filename.ends_with(".png") {
        FileType::Png
    } else if filename.ends_with(".gif") {
        FileType::Gif
    } else {
        FileType::PlainText
    }
}

async fn print_requesthdrs(reader: impl AsyncRead + BufRead) {
    let lines = io::lines(reader);

    await!(lines
        .compat()
        .filter_map(|l| ready(l.ok()))
        .take_while(|l| ready(!l.is_empty()))
        .for_each(|l| ready(println!("{}", l))));

    println!();
}

fn parse_uri(uri: &str) -> Option<String> {
    lazy_static! {
        static ref REGEX: Regex = Regex::new(r"^(?:http://)?(?:.*?)(/.*?)\s*$").unwrap();
    }

    REGEX
        .captures(uri)
        .and_then(|caps| caps.get(1))
        .map(|s| ".".to_string() + s.as_str())
}

async fn client_error(writer: impl AsyncWrite, e: HttpError) -> Result<(), io::Error> {
    let info: (u16, String, String, String) = match e {
        HttpError::Error(e) => (400, "Error".to_string(), "Error occured".to_string(), e),
        HttpError::Forbidden(e) => (
            403,
            "Forbidden".to_string(),
            "The requested file is forbidden".to_string(),
            e,
        ),
        HttpError::IsDirectory(e) => (
            403,
            "Forbidden".to_string(),
            "The requested file is a directory".to_string(),
            e,
        ),
        HttpError::NotFound(e) => (
            404,
            "NotFound".to_string(),
            "The requested file is not found".to_string(),
            e,
        ),
        HttpError::NotImplemented(e) => (
            501,
            "NotImplemented".to_string(),
            "The requested method is not implemented".to_string(),
            e,
        ),
    };

    let body = format!(
        "<html><head><title>Mini Error</title></head><body bgcolor=ffffff>\r\n\
         <b>{}: {}</b>\r\n\
         <p>{}: {}\r\n\
         <hr><em>Mini Web server</em></body></html>\r\n",
        info.0, info.1, info.2, info.3,
    );

    let line = format!("HTTP/1.0 {} {}\r\n", info.0, info.1);

    let header = format!(
        "Content-type: text/html\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );

    let (writer, _) = await!(io::write_all(writer, line).compat())?;
    let (writer, _) = await!(io::write_all(writer, header).compat())?;
    let _ = await!(io::write_all(writer, body).compat())?;

    println!(
        "client error:\n{} {}\n{}: {}\n",
        info.0, info.1, info.2, info.3
    );

    Ok(())
}

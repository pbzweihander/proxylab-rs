extern crate futures;
extern crate regex;
extern crate tokio;
#[macro_use]
extern crate lazy_static;

use futures::future::{err, ok, result, Either};
use regex::Regex;
use std::env::args;
use std::io::{BufRead, BufReader};
use tokio::fs;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

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

    let listener =
        TcpListener::bind(&addr).expect(&format!("unable to bind TCP listener on {}", addr));

    let server = listener
        .incoming()
        .map_err(|e| eprintln!("accept failed: {:?}", e))
        .for_each(|sock| {
            let handler = doit(sock);

            tokio::spawn(handler)
        });

    tokio::run(server);
}

fn doit(sock: TcpStream) -> impl Future<Item = (), Error = ()> {
    let (reader, writer) = sock.split();
    let reader = BufReader::new(reader);

    let reader_future = io::read_until(reader, b'\n', vec![])
        .map_err(|e| HttpError::Error(format!("read failed: {:?}", e)))
        .and_then(|(reader, buf)| {
            let line = String::from_utf8(buf)
                .map_err(|e| HttpError::Error(format!("decode request failed: {:?}", e)));

            let request_line = line
                .and_then(|line| {
                    let mut iter = line.split_whitespace();

                    let method = iter.next();
                    let uri = iter.next();
                    let version = iter.next();

                    method
                        .and_then(|m| {
                            uri.and_then(|u| {
                                version.map(|v| (m.to_string(), u.to_string(), v.to_string()))
                            })
                        }).ok_or(HttpError::Error("request line parsing failed".to_string()))
                }).and_then(|(method, uri, version)| {
                    if method != "GET" {
                        Err(HttpError::NotImplemented(method.to_string()))
                    } else {
                        Ok((method, uri, version))
                    }
                });
            let request_line = result(request_line);

            request_line.map(|r| (reader, r))
        }).and_then(|(reader, (_, uri, _))| print_requesthdrs(reader).map(|_| uri))
        .and_then(|uri| parse_uri(&uri).ok_or(HttpError::Error("uri parsing failed".to_string())))
        .and_then(|filename| {
            let filename1 = filename.clone();
            fs::metadata(filename.clone())
                .map_err(move |e| {
                    use io::ErrorKind;
                    match e.kind() {
                        ErrorKind::NotFound => HttpError::NotFound(filename1),
                        ErrorKind::PermissionDenied => HttpError::Forbidden(filename1),
                        _ => HttpError::Error(format!("file metadata error: {:?}", e)),
                    }
                }).and_then(move |metadata| {
                    if metadata.is_dir() {
                        err(HttpError::IsDirectory(filename))
                    } else {
                        ok((filename, metadata.len()))
                    }
                })
        });

    let serve_future = reader_future.then(|file| match file {
        Ok((filename, size)) => Either::A(serve_static(writer, filename, size)),
        Err(e) => Either::B(client_error(writer, e)),
    });

    serve_future.map_err(|e| eprintln!("io error: {:?}", e))
}

fn serve_static(
    writer: impl AsyncWrite,
    filename: String,
    size: u64,
) -> impl Future<Item = (), Error = io::Error> {
    let file_future = fs::File::open(filename.clone());

    let write_future = file_future.and_then(move |file| {
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

        io::write_all(writer, line)
            .and_then(|(writer, _)| io::write_all(writer, header))
            .and_then(|(writer, _)| io::copy(file, writer))
            .map(|_| ())
    });

    write_future
}

fn get_filetype(filename: &str) -> FileType {
    if filename.ends_with(".html") {
        return FileType::Html;
    } else if filename.ends_with(".jpg") {
        return FileType::Jpg;
    } else if filename.ends_with(".png") {
        return FileType::Png;
    } else if filename.ends_with(".gif") {
        return FileType::Gif;
    } else {
        return FileType::PlainText;
    }
}

fn print_requesthdrs(
    reader: impl AsyncRead + BufRead,
) -> impl Future<Item = (), Error = HttpError> {
    let lines = io::lines(reader);

    lines
        .take_while(|l| ok(!l.is_empty()))
        .for_each(|l| ok(println!("{}", l)))
        .map(|_| println!(""))
        .map_err(|e| HttpError::Error(format!("header reading failed: {:?}", e)))
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

fn client_error(
    writer: impl AsyncWrite,
    e: HttpError,
) -> impl Future<Item = (), Error = io::Error> {
    let info = match e {
        HttpError::Error(e) => (400, "Error", "Error occured", e),
        HttpError::Forbidden(e) => (403, "Forbidden", "The requested file is forbidden", e),
        HttpError::IsDirectory(e) => (403, "Forbidden", "The requested file is a directory", e),
        HttpError::NotFound(e) => (404, "NotFound", "The requested file is not found", e),
        HttpError::NotImplemented(e) => (
            501,
            "NotImplemented",
            "The requested method is not implemented",
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

    io::write_all(writer, line)
        .and_then(|(writer, _)| io::write_all(writer, header))
        .and_then(|(writer, _)| io::write_all(writer, body))
        .map(|_| ())
}

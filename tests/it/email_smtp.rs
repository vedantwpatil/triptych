use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use triptych::email::config::SmtpConfig;
use triptych::email::smtp::{
    OutgoingMessage, base64_encode, build_mime, dot_stuff, expect, parse_recipients, read_reply,
    send, send_line,
};

fn config(port: u16) -> SmtpConfig {
    SmtpConfig {
        account: "work".into(),
        smtp_server: "127.0.0.1".into(),
        smtp_port: port,
        smtp_username: "me@example.com".into(),
        smtp_password: "pw".into(),
        from_addr: "me@example.com".into(),
        signature: None,
    }
}

fn message() -> OutgoingMessage {
    OutgoingMessage {
        to: "you@example.com".into(),
        subject: "Hi".into(),
        body: "one\ntwo".into(),
        ..Default::default()
    }
}

async fn reply(raw: &[u8]) -> anyhow::Result<(u16, String)> {
    read_reply(&mut &raw[..]).await
}

#[tokio::test]
async fn a_reply_is_the_code_and_text_of_its_final_line() {
    assert_eq!(
        reply(b"220 ready\r\n").await.unwrap(),
        (220, "ready".into())
    );
    let multi = reply(b"250-first\r\n250-second\r\n250 last one\r\n").await;
    assert_eq!(multi.unwrap(), (250, "last one".into()));
}

#[tokio::test]
async fn a_reply_cut_short_or_without_a_code_is_an_error() {
    let cut = reply(b"250 ok").await.unwrap_err().to_string();
    assert!(cut.contains("closed the connection"), "{cut}");
    let bad = reply(b"hello\r\n").await.unwrap_err().to_string();
    assert!(bad.contains("malformed"), "{bad}");
}

#[tokio::test]
async fn only_two_and_three_hundred_codes_satisfy_expect() {
    for ok in ["200 a", "250 ok", "354 go", "399 edge"] {
        expect(&mut format!("{ok}\r\n").as_bytes(), "step")
            .await
            .unwrap();
    }
    for (bad, want) in [
        ("199 low", "step rejected: SMTP 199 low"),
        ("400 later", "step rejected: SMTP 400 later"),
        ("550 no user", "step rejected: SMTP 550 no user"),
    ] {
        let err = expect(&mut format!("{bad}\r\n").as_bytes(), "step")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, want);
    }
}

#[tokio::test]
async fn a_command_goes_out_with_a_crlf() {
    let mut out = Vec::new();
    send_line(&mut out, "EHLO triptych").await.unwrap();
    assert_eq!(out, b"EHLO triptych\r\n");
}

fn header<'a>(mime: &'a str, name: &str) -> Option<&'a str> {
    mime.split("\r\n\r\n")
        .next()?
        .lines()
        .find_map(|l| l.strip_prefix(name))
}

#[test]
fn the_message_has_its_headers_then_a_crlf_body() {
    let reply_to = OutgoingMessage {
        cc: "  ".into(),
        in_reply_to: Some("<a@x>".into()),
        references: Some("<z@x> <a@x>".into()),
        ..message()
    };
    let mime = build_mime(&config(587), &reply_to);

    assert_eq!(mime.split_once("\r\n\r\n").unwrap().1, "one\r\ntwo");
    for (name, value) in [
        ("From: ", "me@example.com"),
        ("To: ", "you@example.com"),
        ("Subject: ", "Hi"),
        ("In-Reply-To: ", "<a@x>"),
        ("References: ", "<z@x> <a@x>"),
        ("MIME-Version: ", "1.0"),
    ] {
        assert_eq!(header(&mime, name), Some(value), "{name}");
    }
    assert_eq!(header(&mime, "Cc: "), None, "blank Cc is left out");

    let with_cc = OutgoingMessage {
        cc: "c@x.com".into(),
        ..message()
    };
    assert_eq!(
        header(&build_mime(&config(587), &with_cc), "Cc: "),
        Some("c@x.com")
    );

    // Contact autofill leaves ", " after an accepted entry; it must not reach the headers.
    let trailing = OutgoingMessage {
        to: "Ann <a@x.com>, ".into(),
        cc: "c@x.com; ".into(),
        ..message()
    };
    let mime = build_mime(&config(587), &trailing);
    assert_eq!(header(&mime, "To: "), Some("Ann <a@x.com>"));
    assert_eq!(header(&mime, "Cc: "), Some("c@x.com"));
}

#[test]
fn every_message_gets_its_own_id_at_the_servers_domain() {
    let id = |m: &str| header(m, "Message-ID: ").unwrap().to_string();
    let a = id(&build_mime(&config(587), &message()));
    let b = id(&build_mime(&config(587), &message()));

    assert!(a.starts_with('<') && a.ends_with("@127.0.0.1>"), "{a}");
    assert_ne!(a, b);
}

#[tokio::test]
async fn sending_to_nobody_fails_before_connecting() {
    let nobody = OutgoingMessage {
        to: " ; ".into(),
        ..message()
    };
    let err = send(&config(1), &nobody).await.unwrap_err().to_string();
    assert!(err.contains("no recipients"), "{err}");
}

#[tokio::test]
async fn an_unreachable_server_is_an_error() {
    let err = send(&config(1), &message()).await.unwrap_err().to_string();
    assert!(err.contains("failed to connect"), "{err}");
}

/// Any port but 465 starts in plaintext: greeting, `EHLO`, `STARTTLS`, and only then the handshake.
#[tokio::test]
async fn a_port_other_than_465_asks_for_starttls_in_the_clear() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut seen = Vec::new();
        write.write_all(b"220 ready\r\n").await.unwrap();
        for answer in ["250 hello\r\n", "220 go ahead\r\n"] {
            seen.push(lines.next_line().await.unwrap().unwrap());
            write.write_all(answer.as_bytes()).await.unwrap();
        }
        seen
    });

    let err = send(&config(port), &message())
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("TLS handshake"), "{err}");
    assert_eq!(server.await.unwrap(), ["EHLO triptych", "STARTTLS"]);
}

#[test]
fn base64_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[test]
fn dot_stuff_escapes_leading_dot() {
    assert_eq!(dot_stuff("hello\r\n.\r\nworld"), "hello\r\n..\r\nworld");
    assert_eq!(dot_stuff("no dots here"), "no dots here");
    assert_eq!(dot_stuff(".leading"), "..leading");
}

#[test]
fn parse_recipients_strips_display_names() {
    let got = parse_recipients("Jane Doe <jane@example.com>, bob@example.com", "");
    assert_eq!(got, vec!["jane@example.com", "bob@example.com"]);
}

#[test]
fn parse_recipients_merges_to_and_cc() {
    let got = parse_recipients("a@example.com", "b@example.com; c@example.com");
    assert_eq!(got, vec!["a@example.com", "b@example.com", "c@example.com"]);
}

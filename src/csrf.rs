use axum_extra::extract::cookie::CookieJar;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

pub const COOKIE_NAME: &str = "csrf";

pub fn new_token(secret: &str) -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    format!("{nonce}.{}", signature(secret, &nonce))
}

pub fn valid_token(token: &str, secret: &str) -> bool {
    let Some((nonce, tag)) = token.split_once('.') else { return false; };
    let Some(tag) = decode_hex(tag) else { return false; };
    if nonce.len() != 32 { return false; }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(b"tg-voting/csrf/v1:"); mac.update(nonce.as_bytes());
    mac.verify_slice(&tag).is_ok()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 { return None; }
    (0..value.len()).step_by(2).map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok()).collect()
}

fn signature(secret: &str, nonce: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(b"tg-voting/csrf/v1:"); mac.update(nonce.as_bytes());
    mac.finalize().into_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn token(cookies: &CookieJar) -> Option<&str> { cookies.get(COOKIE_NAME).map(|cookie| cookie.value()) }

pub fn inject_post_tokens(html: &str, cookies: &CookieJar) -> String {
    let Some(token) = token(cookies) else { return html.to_string(); };
    let escaped = token.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    let hidden = format!("<input type=\"hidden\" name=\"csrf_token\" value=\"{escaped}\">");
    let mut output = String::with_capacity(html.len() + hidden.len() * 4);
    let mut remaining = html;
    while let Some(start) = remaining.find("<form") {
        let (before, form) = remaining.split_at(start); output.push_str(before);
        let Some(end) = form.find('>') else { output.push_str(form); return output; };
        let (opening, after) = form.split_at(end + 1); output.push_str(opening);
        let post = opening.contains("method=\"post\"") || opening.contains("method='post'");
        let form_body = after.split_once("</form>").map(|(body, _)| body).unwrap_or(after);
        if post && !form_body.contains("name=\"csrf_token\"") && !form_body.contains("name='csrf_token'") { output.push_str(&hidden); }
        remaining = after;
    }
    output.push_str(remaining); output
}

pub fn form_token_is_valid(body: &[u8], expected: &str) -> bool {
    let Ok(form) = std::str::from_utf8(body) else { return false; };
    form.split('&').any(|pair| { let Some((name, value)) = pair.split_once('=') else { return false; }; name == "csrf_token" && urlencoding::decode(value).ok().is_some_and(|value| value == expected) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_post_forms_receive_one_token() {
        let jar = CookieJar::new().add(axum_extra::extract::cookie::Cookie::new(COOKIE_NAME, "token"));
        let html = inject_post_tokens(
            "<form method=\"get\"></form><form class=\"x\" method=\"post\"></form><form method=\"post\"><input name=\"csrf_token\"></form>",
            &jar,
        );
        assert_eq!(html.matches("csrf_token").count(), 2);
        assert!(form_token_is_valid(b"csrf_token=token", "token"));
    }

    #[test]
    fn oauth_secret_authenticates_tokens() {
        let token = new_token("oauth-secret");
        assert!(valid_token(&token, "oauth-secret"));
        assert!(!valid_token(&token, "other-secret"));
        let mut altered = token.into_bytes();
        altered[0] = b'x';
        assert!(!valid_token(std::str::from_utf8(&altered).unwrap(), "oauth-secret"));
    }
}

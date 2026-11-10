//! Pure, finite redirect Layer. The terminal HTTP Driver remains single-hop.

use crate::{
    Command, CommandKind, EffectCapability, EffectContinuation, EffectOutcome, HttpError,
    HttpErrorKind, HttpRequest, HttpResponse, Perform,
};

// Reuse reqwest's public URL value for pure parsing/resolution only. No client
// or runtime handle crosses this Layer boundary.
use reqwest::Url;

type Finish<Message> = Box<dyn FnOnce(EffectOutcome<HttpResponse, HttpError>) -> Message + Send>;

pub(crate) fn command<Message: Send + 'static>(
    capability: EffectCapability<HttpRequest>,
    request: HttpRequest,
    remaining: usize,
    finish: Finish<Message>,
) -> Command<Message> {
    Command(CommandKind::Effect(Box::new(Perform {
        direct_message: false,
        capability: capability.token.clone(),
        effect: request.clone(),
        map: move |outcome| {
            let outcome = match outcome {
                EffectOutcome::Succeeded(response) => {
                    match next_request(&request, &response, remaining) {
                        Ok(Some(next)) => {
                            return EffectContinuation::Command(command(
                                capability,
                                next,
                                remaining - 1,
                                finish,
                            ));
                        }
                        Ok(None) => EffectOutcome::Succeeded(response),
                        Err(error) => EffectOutcome::Failed(error),
                    }
                }
                other => other,
            };
            EffectContinuation::Message(finish(outcome))
        },
    })))
}

fn failure(message: &str) -> HttpError {
    HttpError::new(HttpErrorKind::Redirect, message)
}

fn next_request(
    request: &HttpRequest,
    response: &HttpResponse,
    remaining: usize,
) -> Result<Option<HttpRequest>, HttpError> {
    let status = response.status().as_u16();
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let mut locations = response.headers().get_all(http::header::LOCATION).iter();
    let Some(location) = locations.next() else {
        return Ok(None);
    };
    if locations.next().is_some() {
        return Err(failure("multiple Location headers"));
    }
    if remaining == 0 {
        return Err(failure("redirect hop limit exceeded"));
    }
    let location = location
        .to_str()
        .map_err(|_| failure("invalid Location header"))?;
    // URL parsers can silently remove ASCII controls. Reject these rather than
    // following a destination different from the declared field value.
    if location.chars().any(|c| c.is_ascii_control() || c == ' ') {
        return Err(failure("invalid whitespace in Location header"));
    }
    let current = Url::parse(request.url()).map_err(|_| failure("invalid redirect base URL"))?;
    let mut target = current
        .join(location)
        .map_err(|_| failure("invalid redirect target URL"))?;
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err(failure("redirect target must use HTTP or HTTPS"));
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(failure("redirect target must not contain URL credentials"));
    }
    if current.scheme() == "https" && target.scheme() == "http" {
        return Err(failure("HTTPS to HTTP redirect rejected"));
    }
    target.set_fragment(None);
    let changes_origin = current.origin() != target.origin();
    let mut next = request.clone();
    next.url = target.as_str().into();
    for name in ["host", "referer", "proxy-authorization"] {
        next.headers.remove(name);
    }
    if changes_origin {
        let sensitive: Vec<_> = next
            .headers
            .iter()
            .filter(|(_, value)| value.is_sensitive())
            .map(|(name, _)| name.clone())
            .collect();
        for name in sensitive {
            next.headers.remove(name);
        }
        for name in ["authorization", "cookie", "cookie2"] {
            next.headers.remove(name);
        }
    }
    if status == 303 || (matches!(status, 301 | 302) && next.method == http::Method::POST) {
        if next.method != http::Method::HEAD {
            next.method = http::Method::GET;
        }
        next.body = bytes::Bytes::new();
        let content_headers: Vec<_> = next
            .headers
            .keys()
            .filter(|name| name.as_str().starts_with("content-"))
            .cloned()
            .collect();
        for name in content_headers {
            next.headers.remove(name);
        }
        for name in ["transfer-encoding", "trailer", "expect"] {
            next.headers.remove(name);
        }
    }
    Ok(Some(next))
}

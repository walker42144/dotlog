use dotlog_memos::browser::BrowserClient;
use dotlog_plugin_sdk::{HttpClient, HttpRequest, HttpResponse, PluginError};
use std::sync::Mutex;
struct Fake {
    calls: Mutex<Vec<(String, String, String)>>,
    invalid: bool,
}
impl HttpClient for Fake {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        let auth = request
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        self.calls
            .lock()
            .unwrap()
            .push((request.method.into(), request.url.into(), auth));
        let (status, body) = if request.url.ends_with("/api/v1/auth/status") {
            (200, r#"{"name":"users/1","username":"alice"}"#)
        } else if request.method == "GET" {
            (
                200,
                if self.invalid {
                    "<html>login</html>"
                } else {
                    r#"{"memos":[{"name":"memos/abc","content":"private note","visibility":"PRIVATE"}],"nextPageToken":"a+b/="}"#
                },
            )
        } else {
            (
                200,
                r#"{"name":"memos/abc","content":"saved","visibility":"PRIVATE"}"#,
            )
        };
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }
}
#[test]
fn browse_authenticates_with_header_and_encodes_pagination_without_exposing_token() {
    let fake = Fake {
        calls: Mutex::new(vec![]),
        invalid: false,
    };
    let client = BrowserClient {
        http: &fake,
        base: "https://memos.test",
        token: "private-token",
    };
    let page = client.browse("v1", Some("a+b/=")).unwrap();
    assert_eq!(page.user, "alice");
    assert_eq!(page.memos.len(), 1);
    let serialized = serde_json::to_string(&page).unwrap();
    assert!(!serialized.contains("private-token"));
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls[0].0, "GET");
    assert!(calls.last().unwrap().1.contains("a%2Bb%2F%3D"));
    assert!(calls.iter().all(|c| c.2 == "Bearer private-token"));
}
#[test]
fn html_login_page_is_an_error_not_empty_success() {
    let fake = Fake {
        calls: Mutex::new(vec![]),
        invalid: true,
    };
    let client = BrowserClient {
        http: &fake,
        base: "https://memos.test",
        token: "token",
    };
    assert!(client.browse("v1", None).is_err());
}
#[test]
fn invalid_remote_memo_identifier_cannot_escape_api_path() {
    let fake = Fake {
        calls: Mutex::new(vec![]),
        invalid: false,
    };
    let client = BrowserClient {
        http: &fake,
        base: "https://memos.test",
        token: "token",
    };
    assert!(client.delete("v1", "../../users/1").is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
}
#[test]
fn empty_post_is_rejected_without_network_request() {
    let fake = Fake {
        calls: Mutex::new(vec![]),
        invalid: false,
    };
    let client = BrowserClient {
        http: &fake,
        base: "https://memos.test",
        token: "token",
    };
    assert!(client.save("v1", None, "  ", "PRIVATE").is_err());
    assert!(fake.calls.lock().unwrap().is_empty());
}

struct IdentityEndpoint {
    path: &'static str,
    body: &'static str,
    status: u16,
}
impl HttpClient for IdentityEndpoint {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        let matched = request.url.ends_with(self.path);
        Ok(HttpResponse {
            status: if matched { self.status } else { 404 },
            body: if matched {
                self.body.as_bytes().to_vec()
            } else {
                b"{}".to_vec()
            },
        })
    }
}
#[test]
fn supports_modern_identity_endpoints_and_nested_user() {
    for path in ["/api/v1/auth/me", "/api/v1/auth/sessions/current"] {
        let http = IdentityEndpoint {
            path,
            body: r#"{"user":{"name":"users/1","username":"alice"}}"#,
            status: 200,
        };
        let client = BrowserClient {
            http: &http,
            base: "https://memos.test",
            token: "secret",
        };
        let (version, user) = client.authenticate("v1").unwrap();
        assert_eq!(version, "v1");
        assert_eq!(user["name"], "users/1");
    }
}
#[test]
fn modern_endpoint_does_not_accept_expired_token_or_generic_success() {
    for (status, body) in [
        (401, "{}"),
        (200, r#"{"name":"login"}"#),
        (200, "<html>login</html>"),
    ] {
        let http = IdentityEndpoint {
            path: "/api/v1/auth/me",
            body,
            status,
        };
        let client = BrowserClient {
            http: &http,
            base: "https://memos.test",
            token: "secret",
        };
        assert!(client.authenticate("v1").is_err());
    }
}

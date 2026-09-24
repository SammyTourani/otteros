use crate::{Url, Method};

/// Redirect policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedirectPolicy {
    /// Maximum number of redirects to follow (None = never follow)
    pub max_redirects: Option<usize>,
}

impl RedirectPolicy {
    /// Create a policy that follows up to 5 redirects
    pub fn follow_up_to_five() -> Self {
        RedirectPolicy { max_redirects: Some(5) }
    }

    /// Create a policy that never follows redirects
    pub fn never() -> Self {
        RedirectPolicy { max_redirects: None }
    }

    /// Check if we should follow this redirect
    pub fn should_follow(&self, count: usize) -> bool {
        match self.max_redirects {
            Some(max) => count < max,
            None => false,
        }
    }
}

impl Default for RedirectPolicy {
    fn default() -> Self {
        Self::follow_up_to_five()
    }
}

/// Determine if a response should trigger a redirect and return the new URL and method
///
/// Per RFC 7231 section 6.4:
/// - 301 (Moved Permanently): GET, change POST to GET
/// - 302 (Found): GET, change POST to GET
/// - 303 (See Other): always becomes GET
/// - 307 (Temporary Redirect): keep method as-is
/// - 308 (Permanent Redirect): keep method as-is
pub fn should_redirect(
    status_code: u16,
    location_header: Option<&str>,
    request_url: &Url,
    request_method: Method,
) -> Option<(Url, Method)> {
    let location = location_header?;
    let new_url = request_url.resolve(location).ok()?;

    let new_method = match status_code {
        301 | 302 => {
            // 301/302: GET always (except for HEAD)
            if request_method == Method::Head {
                Method::Head
            } else {
                Method::Get
            }
        }
        303 => {
            // 303: always GET (except for HEAD)
            if request_method == Method::Head {
                Method::Head
            } else {
                Method::Get
            }
        }
        307 | 308 => {
            // 307/308: keep method
            request_method
        }
        _ => return None,
    };

    Some((new_url, new_method))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_301_get_to_get() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (new_url, new_method) = should_redirect(301, Some("http://example.com/new"), &base, Method::Get).unwrap();

        assert_eq!(new_url.path, "/new");
        assert_eq!(new_method, Method::Get);
    }

    #[test]
    fn test_redirect_302_post_to_get() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (_, new_method) = should_redirect(302, Some("http://example.com/new"), &base, Method::Post).unwrap();

        assert_eq!(new_method, Method::Get);
    }

    #[test]
    fn test_redirect_303_to_get() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (_, new_method) = should_redirect(303, Some("http://example.com/new"), &base, Method::Post).unwrap();

        assert_eq!(new_method, Method::Get);
    }

    #[test]
    fn test_redirect_307_keeps_method() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (_, new_method) = should_redirect(307, Some("http://example.com/new"), &base, Method::Post).unwrap();

        assert_eq!(new_method, Method::Post);
    }

    #[test]
    fn test_redirect_308_keeps_method() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (_, new_method) = should_redirect(308, Some("http://example.com/new"), &base, Method::Post).unwrap();

        assert_eq!(new_method, Method::Post);
    }

    #[test]
    fn test_redirect_head_preserved() {
        let base = Url::parse("http://example.com/old").unwrap();
        let (_, new_method) = should_redirect(301, Some("http://example.com/new"), &base, Method::Head).unwrap();

        assert_eq!(new_method, Method::Head);
    }

    #[test]
    fn test_redirect_no_location() {
        let base = Url::parse("http://example.com/old").unwrap();
        let result = should_redirect(301, None, &base, Method::Get);

        assert!(result.is_none());
    }

    #[test]
    fn test_redirect_relative_url() {
        let base = Url::parse("http://example.com/dir/old").unwrap();
        let (new_url, _) = should_redirect(301, Some("/new"), &base, Method::Get).unwrap();

        assert_eq!(new_url.path, "/new");
    }

    #[test]
    fn test_redirect_policy() {
        let policy = RedirectPolicy::follow_up_to_five();
        assert!(policy.should_follow(0));
        assert!(policy.should_follow(4));
        assert!(!policy.should_follow(5));
    }

    #[test]
    fn test_redirect_policy_never() {
        let policy = RedirectPolicy::never();
        assert!(!policy.should_follow(0));
    }
}

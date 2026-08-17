//  Foundation (fake) — URL / URLComponents / UIImage / NotificationCenter,
//  just enough for the views and the Core handlers.
//

use motor::combine::PassthroughSubject;

/// `Foundation.URL`
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct URL {
    pub absoluteString: String,
}

impl URL {
    /// `URL(string:)` — accepts anything (no real validation).
    pub fn new(string: String) -> Option<URL> {
        Some(URL { absoluteString: string })
    }
}

/// `Foundation.URLQueryItem`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct URLQueryItem {
    pub name: String,
    pub value: Option<String>,
}

/// `Foundation.URLComponents` (just enough: host + query)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct URLComponents {
    pub host: Option<String>,
    pub queryItems: Option<Vec<URLQueryItem>>,
}

impl URLComponents {
    /// `URLComponents(url:resolvingAgainstBaseURL:)`
    pub fn new(url: &URL, _resolvingAgainstBaseURL: bool) -> Option<URLComponents> {
        let rest = url.absoluteString.split_once("://")?.1;
        let (beforeQuery, query) = match rest.split_once('?') {
            Some((before, after)) => (before, Some(after)),
            None => (rest, None),
        };
        let host = beforeQuery.split('/').next().map(str::to_string);
        let queryItems = query.map(|query| {
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((name, value)) => URLQueryItem {
                        name: name.to_string(),
                        value: Some(value.to_string()),
                    },
                    None => URLQueryItem { name: pair.to_string(), value: None },
                })
                .collect()
        });
        Some(URLComponents { host, queryItems })
    }
}

/// `UIKit.UIImage`
#[derive(Clone, Debug, PartialEq)]
pub struct UIImage;

/// `UIKit.UIOpenURLContext`
#[derive(Clone, Debug)]
pub struct UIOpenURLContext {
    pub url: URL,
}

/// `UIKit.UIBackgroundFetchResult`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UIBackgroundFetchResult {
    NewData,
    NoData,
    Failed,
}

/// `Foundation.NotificationCenter.default` (fake) — only the keyboard matters.
/// `keyboardWillShowNotification`/`keyboardWillHideNotification` collapse into
/// a single height subject (hide sends 0), like Swift's `Publishers.Merge`.
pub struct NotificationCenter;

thread_local! {
    static KEYBOARD_HEIGHT: PassthroughSubject<f64> = PassthroughSubject::new();
}

impl NotificationCenter {
    /// `NotificationCenter.default`
    pub fn default() -> NotificationCenter {
        NotificationCenter
    }

    /// `NotificationCenter.default.keyboardHeightPublisher`
    pub fn keyboardHeightPublisher(&self) -> PassthroughSubject<f64> {
        KEYBOARD_HEIGHT.with(|subject| subject.clone())
    }

    /// Simulates `keyboardWillShowNotification` (height) or
    /// `keyboardWillHideNotification` (0) arriving from UIKit.
    pub fn postKeyboardHeight(&self, height: f64) {
        KEYBOARD_HEIGHT.with(|subject| subject.send(height));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlString() {
        let url = URL::new("https://flagcdn.com/w640/us.jpg".into()).unwrap();
        assert_eq!(url.absoluteString, "https://flagcdn.com/w640/us.jpg");
    }

    #[test]
    fn urlComponentsParsesHostAndQuery() {
        let url = URL::new("https://www.example.com/country?alpha3code=USA&extra".into()).unwrap();
        let components = URLComponents::new(&url, true).unwrap();
        assert_eq!(components.host.as_deref(), Some("www.example.com"));
        let query = components.queryItems.unwrap();
        assert_eq!(query[0].name, "alpha3code");
        assert_eq!(query[0].value.as_deref(), Some("USA"));
        assert_eq!(query[1].value, None);
    }

    #[test]
    fn keyboardObserverDelivers() {
        let heights = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = heights.clone();
        NotificationCenter::default()
            .keyboardHeightPublisher()
            .sink(move |height| seen.borrow_mut().push(*height))
            .store_in(&motor::cancel_bag::CancelBag::new());
        NotificationCenter::default().postKeyboardHeight(216.0);
        NotificationCenter::default().postKeyboardHeight(0.0);
        assert_eq!(*heights.borrow(), vec![216.0, 0.0]);
    }
}

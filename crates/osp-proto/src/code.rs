//! OSP codes (ADR 0039 §4): a profile or a post as one copyable string.
//!
//! - A profile: `osp:person/<invite code>`, the follow ticket with a prefix.
//! - A post: `osp:post/<post ID>?by=<invite code>`, the post's ID and its
//!   author's follow ticket.
//!
//! A code holds the prefix, an invite code and a post ID, and nothing else.
//! Parsing is strict: a code with parts this version doesn't know is refused,
//! with a message in plain words, rather than read in part.

use std::{fmt, str::FromStr};

use crate::{object::ObjectId, ticket::FollowTicket};

const SCHEME: &str = "osp:";
const PERSON: &str = "person/";
const POST: &str = "post/";
const BY: &str = "by=";

/// Longest code `find_in` looks at. A real one is a few hundred characters;
/// this only stops a huge paste from being decoded.
const MAX_CODE_LEN: usize = 4096;

/// A shared profile or post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OspCode {
    /// Someone's profile: what's needed to follow them.
    Person(FollowTicket),
    /// A post, and how to reach its author to fetch it.
    Post { id: ObjectId, by: FollowTicket },
}

/// Why a string isn't an OSP code, in words the app can show as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    /// Doesn't start with `osp:`.
    NotACode,
    /// `osp:` followed by something other than `person/` or `post/`.
    UnknownKind,
    /// The invite code part doesn't decode.
    BadInvite,
    /// The post ID part isn't 64 hex digits.
    BadPostId,
    /// A post code without `?by=`.
    MissingAuthor,
    /// Anything after the parts a code has: unknown options, a second `?`.
    ExtraParts,
}

impl fmt::Display for CodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotACode => "This isn't an OSP code. OSP codes start with \"osp:\".",
            Self::UnknownKind => {
                "This OSP code is for something this version of OSP can't open. \
                 An update may open it."
            }
            Self::BadInvite => {
                "The invite code in this OSP code is damaged. \
                 Check it was copied whole, or ask for it again."
            }
            Self::BadPostId => {
                "The post ID in this OSP code is damaged. \
                 Check it was copied whole, or ask for it again."
            }
            Self::MissingAuthor => {
                "This post code doesn't say who wrote the post, so it can't be fetched. \
                 Ask for it again."
            }
            Self::ExtraParts => {
                "This OSP code has parts this version of OSP doesn't know. \
                 An update may open it."
            }
        })
    }
}

impl std::error::Error for CodeError {}

impl OspCode {
    /// The first OSP code in `text` that parses, if any: for a paste or the
    /// clipboard, where a code may come with words around it.
    pub fn find_in(text: &str) -> Option<OspCode> {
        let lower = text.to_ascii_lowercase();
        let mut from = 0;
        while let Some(at) = lower[from..].find(SCHEME) {
            let start = from + at;
            from = start + SCHEME.len();
            let token = text[start..]
                .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
                .next()
                .unwrap_or("");
            let token = token.trim_end_matches(['.', ',', ';', ':', '!', ')', ']']);
            if token.len() <= MAX_CODE_LEN
                && let Ok(code) = token.parse()
            {
                return Some(code);
            }
        }
        None
    }

    /// Whose code it is: the profile's, or the post's author.
    pub fn ticket(&self) -> &FollowTicket {
        match self {
            Self::Person(ticket) => ticket,
            Self::Post { by, .. } => by,
        }
    }
}

impl fmt::Display for OspCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Person(ticket) => write!(f, "{SCHEME}{PERSON}{ticket}"),
            Self::Post { id, by } => write!(f, "{SCHEME}{POST}{id}?{BY}{by}"),
        }
    }
}

impl FromStr for OspCode {
    type Err = CodeError;

    fn from_str(s: &str) -> Result<Self, CodeError> {
        let s = s.trim();
        let rest = strip_prefix_ci(s, SCHEME).ok_or(CodeError::NotACode)?;
        // Some apps turn `osp:x` into `osp://x` when they make it a link.
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        if let Some(invite) = strip_prefix_ci(rest, PERSON) {
            if invite.contains(['?', '/', '#']) {
                return Err(CodeError::ExtraParts);
            }
            return parse_invite(invite).map(Self::Person);
        }
        if let Some(post) = strip_prefix_ci(rest, POST) {
            let (id, query) = post.split_once('?').ok_or(CodeError::MissingAuthor)?;
            if id.contains(['/', '#']) {
                return Err(CodeError::ExtraParts);
            }
            let id: ObjectId = id.parse().map_err(|_| CodeError::BadPostId)?;
            let invite = strip_prefix_ci(query, BY).ok_or(if query.contains('=') {
                CodeError::ExtraParts
            } else {
                CodeError::MissingAuthor
            })?;
            if invite.contains(['?', '&', '/', '#']) {
                return Err(CodeError::ExtraParts);
            }
            return Ok(Self::Post {
                id,
                by: parse_invite(invite)?,
            });
        }
        Err(CodeError::UnknownKind)
    }
}

fn parse_invite(s: &str) -> Result<FollowTicket, CodeError> {
    s.parse().map_err(|_| CodeError::BadInvite)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::{EndpointAddr, SecretKey};

    /// A ticket as a node makes one: node ID and relay URL, no IP address.
    fn ticket() -> FollowTicket {
        FollowTicket {
            actor: SecretKey::generate().public(),
            node: EndpointAddr::new(SecretKey::generate().public())
                .with_relay_url("https://relay.example.org./".parse().unwrap()),
        }
    }

    fn post_id() -> ObjectId {
        ObjectId::of(b"a post")
    }

    #[test]
    fn round_trips() {
        let person = OspCode::Person(ticket());
        assert_eq!(person.to_string().parse::<OspCode>(), Ok(person.clone()));
        let post = OspCode::Post {
            id: post_id(),
            by: ticket(),
        };
        assert_eq!(post.to_string().parse::<OspCode>(), Ok(post.clone()));
        // Case, `//` and spaces around don't matter.
        let shouted = format!("  {}\n", post.to_string().to_uppercase());
        assert_eq!(shouted.parse::<OspCode>(), Ok(post.clone()));
        let slashed = post.to_string().replacen("osp:", "osp://", 1);
        assert_eq!(slashed.parse::<OspCode>(), Ok(post));
    }

    /// Gate (ADR 0039): a code holds only the prefix, an invite code and a
    /// post ID.
    #[test]
    fn gate_a_code_holds_only_prefix_invite_and_post_id() {
        let by = ticket();
        let id = post_id();
        assert_eq!(
            OspCode::Person(by.clone()).to_string(),
            format!("osp:person/{by}")
        );
        assert_eq!(
            OspCode::Post { id, by: by.clone() }.to_string(),
            format!("osp:post/{id}?by={by}")
        );
    }

    /// Gate (ADR 0039): a malformed code is refused with a plain message.
    #[test]
    fn gate_malformed_codes_are_refused_in_plain_words() {
        let by = ticket();
        let id = post_id();
        let cases = [
            ("", CodeError::NotACode),
            ("hello", CodeError::NotACode),
            (&by.to_string(), CodeError::NotACode),
            ("osp:", CodeError::UnknownKind),
            ("osp:group/abc", CodeError::UnknownKind),
            ("osp:person/", CodeError::BadInvite),
            ("osp:person/OSP1AAAA", CodeError::BadInvite),
            (&format!("osp:person/{by}?x=1"), CodeError::ExtraParts),
            (&format!("osp:post/{id}"), CodeError::MissingAuthor),
            (&format!("osp:post/{id}?"), CodeError::MissingAuthor),
            (&format!("osp:post/abc?by={by}"), CodeError::BadPostId),
            (&format!("osp:post/{id}?by="), CodeError::BadInvite),
            (&format!("osp:post/{id}?x=1"), CodeError::ExtraParts),
            (&format!("osp:post/{id}?by={by}&x=1"), CodeError::ExtraParts),
            (
                &format!("osp:post/{id}/more?by={by}"),
                CodeError::ExtraParts,
            ),
        ];
        for (code, want) in cases {
            assert_eq!(code.parse::<OspCode>(), Err(want), "{code}");
            let message = want.to_string();
            assert!(message.ends_with('.'), "{message}");
            assert!(!message.contains("base32") && !message.contains("hex"));
        }
    }

    #[test]
    fn finds_a_code_among_words() {
        let post = OspCode::Post {
            id: post_id(),
            by: ticket(),
        };
        let text = format!(
            "Kim shared a post on Open Social P2P. Get OSP, then open it:\n({post}).\nosp:nothing"
        );
        assert_eq!(OspCode::find_in(&text), Some(post.clone()));
        // A broken code before a good one is skipped.
        let text = format!("osp:person/OSP1AAAA and \"{post}\"");
        assert_eq!(OspCode::find_in(&text), Some(post));
        assert_eq!(OspCode::find_in("no code here, osp: nothing"), None);
    }
}

//! Reading a password export (CSV) from Chrome and other browsers, 1Password
//! or Bitwarden. Columns are found by header name, so one reader covers all
//! three and anything shaped like them. Nothing here touches the vault.

use serde::Serialize;
use zeroize::Zeroizing;

use super::origin;

const MAX_ROWS: usize = 20_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Skip {
    /// Line number in the file (the header is line 1).
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Column {
    /// What Pindot uses it for: "name", "website", "username", "password", "authenticator".
    pub field: String,
    /// The header in the user's file.
    pub header: String,
}

pub struct Row {
    pub line: usize,
    pub name: String,
    pub origin: String,
    pub username: String,
    pub password: Zeroizing<String>,
    pub totp: Option<Zeroizing<String>>,
}

pub struct Parsed {
    pub format: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub skipped: Vec<Skip>,
    /// Rows whose authenticator key was present but not usable (the login
    /// is still imported, without it).
    pub bad_totp: usize,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ImportError {
    #[error("that file is empty")]
    Empty,
    #[error("that does not look like a password export: it needs a website column and a password column")]
    NoColumns,
    #[error("the file could not be read as CSV")]
    Csv,
    #[error("the file has more than {MAX_ROWS} rows")]
    TooBig,
}

const NAME: &[&str] = &["name", "title"];
const URL: &[&str] = &["url", "login_uri", "website", "web site", "login url", "uri"];
const USER: &[&str] = &["username", "login_username", "login", "user", "email", "login email"];
const PASS: &[&str] = &["password", "login_password"];
const TOTP: &[&str] = &["otpauth", "login_totp", "totp", "otp", "one-time password", "one time password"];

fn find(headers: &[String], names: &[&str]) -> Option<usize> {
    names.iter().find_map(|n| headers.iter().position(|h| h == n))
}

pub fn parse_csv(text: &str) -> Result<Parsed, ImportError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Err(ImportError::Empty);
    }
    let mut rdr = csv::ReaderBuilder::new().flexible(true).has_headers(true).from_reader(text.as_bytes());
    let raw_headers: Vec<String> = rdr.headers().map_err(|_| ImportError::Csv)?.iter().map(|h| h.trim().to_string()).collect();
    let headers: Vec<String> = raw_headers.iter().map(|h| h.to_lowercase()).collect();

    let (c_url, c_pass) = (find(&headers, URL), find(&headers, PASS));
    let (Some(c_url), Some(c_pass)) = (c_url, c_pass) else { return Err(ImportError::NoColumns) };
    let c_name = find(&headers, NAME);
    let c_user = find(&headers, USER);
    let c_totp = find(&headers, TOTP);
    let c_type = headers.iter().position(|h| h == "type");
    let c_archived = headers.iter().position(|h| h == "archived");

    let format = if headers.iter().any(|h| h == "login_uri") {
        "Bitwarden"
    } else if c_totp.is_some_and(|i| headers[i] == "otpauth") || c_archived.is_some() {
        "1Password"
    } else {
        "Chrome (or a similar browser export)"
    };

    let mut columns = Vec::new();
    for (field, idx) in [
        ("name", c_name),
        ("website", Some(c_url)),
        ("username", c_user),
        ("password", Some(c_pass)),
        ("authenticator", c_totp),
    ] {
        if let Some(i) = idx {
            columns.push(Column { field: field.into(), header: raw_headers[i].clone() });
        }
    }

    let (mut rows, mut skipped, mut bad_totp) = (Vec::new(), Vec::new(), 0usize);
    for (n, rec) in rdr.records().enumerate() {
        let line = n + 2;
        if n >= MAX_ROWS {
            return Err(ImportError::TooBig);
        }
        let rec = rec.map_err(|_| ImportError::Csv)?;
        let cell = |i: Option<usize>| i.and_then(|i| rec.get(i)).unwrap_or("").trim().to_string();
        let skip = |reason: &str| Skip { line, reason: reason.into() };

        if c_type.is_some() {
            let t = cell(c_type).to_lowercase();
            if !t.is_empty() && t != "login" && t != "password" {
                skipped.push(skip("not a website login"));
                continue;
            }
        }
        if c_archived.is_some() && matches!(cell(c_archived).to_lowercase().as_str(), "true" | "yes" | "1") {
            skipped.push(skip("archived"));
            continue;
        }
        let url_cell = cell(Some(c_url));
        if url_cell.is_empty() {
            skipped.push(skip("no website address"));
            continue;
        }
        let password = Zeroizing::new(cell(Some(c_pass)));
        if password.is_empty() {
            skipped.push(skip("no password"));
            continue;
        }
        // Bitwarden can list several addresses in one cell: the first usable one.
        let mut origin_found = None;
        let mut first_err = None;
        for part in url_cell.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty()) {
            match origin::parse_saved_origin(part) {
                Ok(o) => {
                    origin_found = Some(o);
                    break;
                }
                Err(e) => first_err = first_err.or(Some(e)),
            }
        }
        let Some(origin) = origin_found else {
            let reason = match first_err {
                Some(origin::OriginError::Scheme) => "not a website (an app or other address)",
                Some(origin::OriginError::InsecureHttp) => "plain http site (only https is saved)",
                Some(origin::OriginError::UserInfo) => "address contains a user name",
                _ => "website address not understood",
            };
            skipped.push(skip(reason));
            continue;
        };
        let totp = {
            let t = cell(c_totp);
            if t.is_empty() {
                None
            } else if super::totp::Totp::parse(&t).is_ok() {
                Some(Zeroizing::new(t))
            } else {
                bad_totp += 1;
                None
            }
        };
        let host = origin.split("://").nth(1).unwrap_or(&origin).to_string();
        let name = Some(cell(c_name)).filter(|n| !n.is_empty()).unwrap_or(host);
        rows.push(Row { line, name, origin, username: cell(c_user), password, totp });
    }
    Ok(Parsed { format: format.into(), columns, rows, skipped, bad_totp })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Small fake exports in the shape each tool writes. Every value is made up.
    const CHROME: &str = "name,url,username,password,note\n\
Example,https://example.com/login,ann@example.test,pw-one,\n\
Local,http://localhost:4310/,dev,pw-two,\"a note\nover two lines\"\n\
NoUrl,,bob,pw-three,\n\
NoPass,https://nopass.test,bob,,\n\
Plain,http://plain.test,bob,pw-four,\n\
App,android://hash@com.example.app/,bob,pw-five,\n";

    const ONEPASSWORD: &str = "Title,Url,Username,Password,OTPAuth,Favorite,Archived,Tags,Notes\n\
Work,https://work.example.test/sign-in,ann,pw-work,otpauth://totp/Work:ann?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Work,0,false,,\n\
Old,https://old.example.test,ann,pw-old,,0,true,,\n\
BadKey,https://badkey.example.test,ann,pw-bad,not-a-key!!,0,false,,\n";

    const BITWARDEN: &str = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
,,login,Bank,,,0,https://bank.example.test/a,ann,pw-bank,GEZDGNBVGY3TQOJQ\n\
,,note,Secret note,words,,0,,,,\n\
,,login,Multi,,,0,\"android://x@y\nhttps://multi.example.test:8443/z\",ann,pw-multi,\n\
,,card,Visa,,,0,,,,\n";

    #[test]
    fn chrome_export() {
        let p = parse_csv(CHROME).unwrap();
        assert!(p.format.starts_with("Chrome"));
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].origin, "https://example.com");
        assert_eq!(p.rows[0].username, "ann@example.test");
        assert_eq!(p.rows[1].origin, "http://localhost:4310");
        let reasons: Vec<_> = p.skipped.iter().map(|s| (s.line, s.reason.as_str())).collect();
        // The quoted note spans two lines of text but is one CSV record, so
        // line numbers count records.
        assert_eq!(
            reasons,
            vec![
                (4, "no website address"),
                (5, "no password"),
                (6, "plain http site (only https is saved)"),
                (7, "not a website (an app or other address)"),
            ]
        );
        assert!(p.columns.iter().any(|c| c.field == "website" && c.header == "url"));
    }

    #[test]
    fn onepassword_export_with_totp_and_archived() {
        let p = parse_csv(ONEPASSWORD).unwrap();
        assert_eq!(p.format, "1Password");
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].name, "Work");
        assert!(p.rows[0].totp.is_some());
        assert!(p.rows[1].totp.is_none());
        assert_eq!(p.bad_totp, 1);
        assert_eq!(p.skipped, vec![Skip { line: 3, reason: "archived".into() }]);
    }

    #[test]
    fn bitwarden_export_skips_notes_and_cards() {
        let p = parse_csv(BITWARDEN).unwrap();
        assert_eq!(p.format, "Bitwarden");
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].totp.as_deref().map(String::as_str), Some("GEZDGNBVGY3TQOJQ"));
        assert_eq!(p.rows[1].origin, "https://multi.example.test:8443");
        assert_eq!(p.skipped.len(), 2);
        assert!(p.skipped.iter().all(|s| s.reason == "not a website login"));
    }

    #[test]
    fn not_a_password_file() {
        assert_eq!(parse_csv("").err(), Some(ImportError::Empty));
        assert_eq!(parse_csv("a,b\n1,2\n").err(), Some(ImportError::NoColumns));
        // A byte-order mark in front of the header is fine.
        assert!(parse_csv("\u{feff}url,password\nhttps://a.test,x\n").is_ok());
    }

    #[test]
    fn debug_of_skips_has_no_values() {
        let p = parse_csv(CHROME).unwrap();
        let text = format!("{:?}{:?}", p.skipped, p.columns);
        assert!(!text.contains("pw-"));
    }
}

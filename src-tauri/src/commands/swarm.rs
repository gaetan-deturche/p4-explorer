//! Browsing Swarm code reviews, and turning one into something applicable.
//!
//! The list comes from Swarm's REST API rather than from p4, because only Swarm
//! knows a review's state, its author and its versions. Every filter is passed
//! to the server (`state[]`, `author[]`, `participants[]`, `keywords`) so the
//! tab pages through reviews instead of downloading them all to filter locally.
//!
//! A review's content, though, is plain Perforce: each version is a shelved
//! changelist owned by the `swarm` user, readable with `p4 describe -S`. That
//! is what makes the file list, the diffs and the apply work with the plumbing
//! that already exists — nothing here needs to understand Swarm's diff format.

use crate::p4::{self, P4Conn};

/// One row of the review list.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRow {
    /// "review" for a Swarm review, "shelf" for a shelved changelist that has no
    /// review at all. Both are things you might want to read before they land, so
    /// the tab lists them together — but only one of them has a state, reviewers
    /// or a Swarm page.
    pub kind: String,
    pub id: u64,
    pub state: String,
    pub state_label: String,
    pub author: String,
    pub description: String,
    pub created: i64,
    pub updated: i64,
    /// The changelist holding the current shelf — what to describe, diff and
    /// apply. Swarm keeps the review's own id as a `swarm`-owned changelist whose
    /// shelf tracks the latest version, and the list endpoint does NOT return
    /// `versions` (only the single-review endpoint does), so the id is the
    /// reliable answer; a version list, when present, still wins.
    pub change: u64,
    /// True while the review is still open (not committed/archived).
    pub pending: bool,
    /// Submitted changelists this review landed as. Often non-empty on a review
    /// that is STILL `needsReview`: submitting deletes the shelf, so this is where
    /// the content lives once someone pushed without waiting for approval.
    pub commits: Vec<u64>,
    /// Distinct participants other than the author — reviewers, roughly.
    pub reviewers: Vec<String>,
}

/// A page of reviews plus the cursor to continue from.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPage {
    pub reviews: Vec<ReviewRow>,
    /// Pass back as `after` to fetch the next page; 0 when the end was reached.
    pub last_seen: u64,
    /// Empty on success, else why the list is empty — an unreachable Swarm and
    /// a genuinely empty result must not look the same in the UI.
    pub error: String,
}

/// What the tab is asking for. `role` selects which Swarm filter the `user`
/// goes into: author, participant ("reviewer"), or both.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQuery {
    pub states: Vec<String>,
    pub user: String,
    pub role: String, // "author" | "reviewer" | "any"
    pub keywords: String,
    pub max: u32,
    pub after: u64,
    /// Depot path of the current stream (e.g. `//Curiosity/main`). When set, only
    /// reviews whose changelists live under it are returned. Empty = every review
    /// the server has, whatever depot it is on.
    pub stream_path: String,
    /// Drop reviews that already went in (a non-empty `commits`). Swarm keeps
    /// those at needsReview forever, so they otherwise crowd out the ones that
    /// still want reading.
    pub hide_submitted: bool,
}

/// Human label for a Swarm review state.
fn state_label(state: &str) -> String {
    match state {
        "needsReview" => "Needs Review",
        "needsRevision" => "Needs Revision",
        "approved" => "Approved",
        "approved:commit" => "Approved (commit)",
        "rejected" => "Rejected",
        "archived" => "Archived",
        "requested" => "Review Requested",
        other => {
            let mut c = other.chars();
            return match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            };
        }
    }
    .to_string()
}

/// Swarm base URL with any trailing slash removed (empty if unconfigured).
fn swarm_base(conn: &P4Conn) -> String {
    let out = p4::run_raw(conn, &["property", "-l", "-n", "P4.Swarm.URL"]).unwrap_or_default();
    out.lines()
        .next()
        .and_then(|l| l.splitn(2, '=').nth(1))
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// Percent-encode a query-parameter value. Descriptions and user names reach the
/// URL, so `&`, `#`, spaces and non-ASCII must not be able to split the query.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Every changelist under `path` that could belong to a review: the shelved ones
/// (a review version is a shelf, including Swarm's own shadow changelists) plus
/// recent submitted ones (a committed review points at those instead).
///
/// This is how a review is placed in a stream at all: Swarm's list endpoint
/// returns no stream, and asking it per review would be a round-trip each. One
/// `p4 changes` answers it for two thousand changelists in a fraction of a second.
fn changes_under(conn: &P4Conn, path: &str) -> std::collections::HashSet<u64> {
    let mut out = std::collections::HashSet::new();
    let spec = format!("{}/...", path.trim_end_matches('/'));
    for args in [
        vec!["changes", "-s", "shelved", "-m", "2000", spec.as_str()],
        vec!["changes", "-m", "2000", spec.as_str()],
    ] {
        if let Ok(recs) = p4::run(conn, &args) {
            for r in &recs {
                if let Some(c) = r.get("change").and_then(|v| v.as_str()) {
                    if let Ok(n) = c.parse::<u64>() {
                        out.insert(n);
                    }
                }
            }
        }
    }
    out
}

/// Shelved changelists under `stream_path` that NO review covers.
///
/// Two things make this cheap enough to sit next to the review list. Almost
/// every shelf in a stream belongs to Swarm — measured on this depot, 375 of the
/// 400 most recent were `swarm`-owned shadow changelists, two per review — so
/// dropping that user removes 94% of the rows before anything else runs. And
/// Swarm's `change[]` filter takes a LIST, so one HTTP request says which of the
/// survivors are already in review (each returned review names the changelists
/// it covers), instead of one round-trip per changelist.
///
/// `scan` bounds the `p4 changes` window, not the result.
#[tauri::command]
pub async fn swarm_shelved_no_review(
    conn: P4Conn,
    stream_path: String,
    scan: u32,
) -> Result<Vec<ReviewRow>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let scan = scan.clamp(1, 2000).to_string();
        let spec = if stream_path.trim().is_empty() {
            "//...".to_string()
        } else {
            format!("{}/...", stream_path.trim_end_matches('/'))
        };
        // -l: without it p4 truncates the description to ~31 characters.
        let recs = p4::run(&conn, &["changes", "-l", "-s", "shelved", "-m", &scan, &spec])?;

        struct Shelf {
            change: u64,
            user: String,
            desc: String,
            time: i64,
        }
        let mut shelves: Vec<Shelf> = Vec::new();
        for r in &recs {
            let user = r.get("user").and_then(|v| v.as_str()).unwrap_or("");
            // Swarm's own shadow changelists are the review versions themselves;
            // the review list already represents those.
            if user.is_empty() || user == "swarm" {
                continue;
            }
            let Some(change) = r
                .get("change")
                .and_then(|v| v.as_str())
                .and_then(|c| c.parse::<u64>().ok())
            else {
                continue;
            };
            shelves.push(Shelf {
                change,
                user: user.to_string(),
                desc: r.get("desc").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                time: r
                    .get("time")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0),
            });
        }
        if shelves.is_empty() {
            return Ok(Vec::new());
        }

        // Which of them Swarm already knows about.
        let mut in_review: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let base = swarm_base(&conn);
        if let (false, Some(ticket)) = (base.is_empty(), p4::ticket(&conn)) {
            if let Ok(client) = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
            {
                for batch in shelves.chunks(50) {
                    let mut url = format!("{base}/api/v9/reviews?max=200&fields=id,changes,commits");
                    for s in batch {
                        url.push_str(&format!("&change[]={}", s.change));
                    }
                    let Ok(resp) = client.get(&url).basic_auth(&conn.user, Some(&ticket)).send()
                    else {
                        continue; // Swarm unreachable → treat these as unknown
                    };
                    if !resp.status().is_success() {
                        continue;
                    }
                    let Ok(body) = resp.json::<serde_json::Value>() else { continue };
                    for rv in body.get("reviews").and_then(|r| r.as_array()).into_iter().flatten() {
                        if let Some(id) = rv.get("id").and_then(|v| v.as_u64()) {
                            in_review.insert(id);
                        }
                        for key in ["changes", "commits"] {
                            for c in rv.get(key).and_then(|v| v.as_array()).into_iter().flatten() {
                                if let Some(n) = c.as_u64() {
                                    in_review.insert(n);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(shelves
            .into_iter()
            .filter(|s| !in_review.contains(&s.change))
            .map(|s| ReviewRow {
                kind: "shelf".to_string(),
                id: s.change, // no review id; the changelist identifies the row
                state: String::new(),
                state_label: "Shelved".to_string(),
                author: s.user,
                description: s.desc,
                created: s.time,
                updated: s.time,
                change: s.change,
                pending: true,
                commits: Vec::new(),
                reviewers: Vec::new(),
            })
            .collect())
    })
    .await
    .map_err(|e| format!("shelved-changelist scan failed: {e}"))?
}

/// How many people one half-typed name may stand for. Past this the filter has
/// stopped being a filter, and a URL with hundreds of author[] values is not
/// something to send at a server on the strength of two characters.
const MAX_MATCHED_USERS: usize = 40;

/// Lower-case and strip the accents, so a name typed on a plain keyboard reaches
/// the account it belongs to. Our own list has "Léo-Paul Couturier" in it, and
/// nobody types that é into a filter box.
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ą' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'ę' | 'ě' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' | 'ń' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'ů' => 'u',
            'ý' | 'ÿ' => 'y',
            'ł' => 'l',
            'ś' | 'š' => 's',
            'ź' | 'ż' | 'ž' => 'z',
            'ř' => 'r',
            other => other,
        })
        .collect()
}

/// The user ids a piece of typed text stands for.
///
/// An exact id wins outright — someone who typed a whole id means that person,
/// even if it is also a prefix of somebody else's. Otherwise every typed word has
/// to appear somewhere in the id, the full name or the email: that makes
/// "leo-paul", "couturier", "Leo-Paul Couturier" and "couturier leo" all reach the
/// same account, and it is why the email is included — ours are dotted
/// differently from the ids (leo.paul.couturier@ vs leo-paul.couturier).
fn match_users(users: &[(String, String, String)], typed: &str) -> Vec<String> {
    let needle = fold(typed.trim());
    if needle.is_empty() {
        return Vec::new();
    }
    for (id, _, _) in users {
        if fold(id) == needle {
            return vec![id.clone()];
        }
    }
    let words: Vec<&str> = needle.split_whitespace().collect();
    let mut matches: Vec<String> = Vec::new();
    for (id, full, email) in users {
        let hay = format!("{} {} {}", fold(id), fold(full), fold(email));
        if words.iter().all(|w| hay.contains(w)) {
            matches.push(id.clone());
        }
    }
    matches.truncate(MAX_MATCHED_USERS);
    matches
}

/// `match_users` against the server's own list.
fn resolve_users(conn: &P4Conn, typed: &str) -> Result<Vec<String>, String> {
    let recs = p4::run(conn, &["users"])?;
    let users: Vec<(String, String, String)> = recs
        .iter()
        .filter_map(|r| {
            let id = r.get("User").and_then(|u| u.as_str())?.to_string();
            Some((
                id,
                r.get("FullName").and_then(|f| f.as_str()).unwrap_or("").to_string(),
                r.get("Email").and_then(|e| e.as_str()).unwrap_or("").to_string(),
            ))
        })
        .collect();
    Ok(match_users(&users, typed))
}

#[cfg(test)]
mod user_match_tests {
    use super::match_users;

    /// The real rows, verbatim from `p4 users` on our server.
    fn users() -> Vec<(String, String, String)> {
        [
            ("leo-paul.couturier", "Léo-Paul Couturier", "leo.paul.couturier@sloclap.com"),
            ("gaetan.deturche", "Gaetan DETURCHE", "gaetan.deturche@sloclap.com"),
            ("gaetan.fillardet", "Gaetan Fillardet", "gaetan.fillardet@sloclap.com"),
            ("jerome.charles", "Jerome CHARLES", "jerome.charles@sloclap.com"),
        ]
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect()
    }

    #[test]
    fn a_half_typed_id_finds_the_account() {
        // The bug: "leo-paul" is not "leo-paul.couturier", so Swarm matched
        // nobody and the tab showed an empty list.
        assert_eq!(match_users(&users(), "leo-paul"), vec!["leo-paul.couturier"]);
        assert_eq!(match_users(&users(), "couturier"), vec!["leo-paul.couturier"]);
    }

    #[test]
    fn an_accent_nobody_types_is_folded_away() {
        // The list says "Léo-Paul Couturier"; a filter box gets plain letters.
        assert_eq!(match_users(&users(), "Leo-Paul Couturier"), vec!["leo-paul.couturier"]);
        assert_eq!(match_users(&users(), "léo-paul"), vec!["leo-paul.couturier"]);
    }

    #[test]
    fn the_words_may_come_in_any_order() {
        assert_eq!(match_users(&users(), "couturier leo"), vec!["leo-paul.couturier"]);
    }

    #[test]
    fn an_email_reaches_its_owner() {
        // Ours are dotted differently from the ids, so the id alone is not enough.
        assert_eq!(
            match_users(&users(), "leo.paul.couturier@sloclap.com"),
            vec!["leo-paul.couturier"]
        );
    }

    #[test]
    fn a_name_two_people_share_finds_both() {
        // Swarm ORs repeated author[] values, so both are asked for — measured on
        // the live server (19 + 1 rows for two authors).
        assert_eq!(match_users(&users(), "gaetan"), vec!["gaetan.deturche", "gaetan.fillardet"]);
    }

    #[test]
    fn an_exact_id_means_that_person_alone() {
        // Even when it is a substring of nothing else, the exact match short
        // circuits: someone who typed a whole id has already chosen.
        assert_eq!(match_users(&users(), "gaetan.deturche"), vec!["gaetan.deturche"]);
        assert_eq!(match_users(&users(), "GAETAN.DETURCHE"), vec!["gaetan.deturche"]);
    }

    #[test]
    fn nobody_matches_nonsense() {
        assert!(match_users(&users(), "zzz").is_empty());
        assert!(match_users(&users(), "   ").is_empty());
    }
}

/// A page of reviews matching `query`. Errors are reported in the page rather
/// than thrown, so the tab can say *why* it is empty.
#[tauri::command]
pub async fn swarm_reviews(conn: P4Conn, query: ReviewQuery) -> Result<ReviewPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let empty = |error: &str| ReviewPage {
            reviews: Vec::new(),
            last_seen: 0,
            error: error.to_string(),
        };
        let base = swarm_base(&conn);
        if base.is_empty() {
            return Ok(empty("No Swarm server is configured (P4.Swarm.URL is unset)."));
        }
        let Some(ticket) = p4::ticket(&conn) else {
            return Ok(empty("Not logged in — Swarm needs a valid P4 ticket."));
        };

        // Whom the typed text stands for. Swarm keys on the exact user id, so
        // "leo-paul" matched nobody until this: it is resolved against the user
        // list, and every match is asked for.
        let who: Vec<String> = if query.user.is_empty() {
            Vec::new()
        } else {
            match resolve_users(&conn, &query.user) {
                Ok(list) if list.is_empty() => {
                    return Ok(empty(&format!("No user matches \u{201c}{}\u{201d}.", query.user)))
                }
                Ok(list) => list,
                // The user list could not be read: fall back to the text as
                // typed, which is right whenever it IS an exact id.
                Err(_) => vec![query.user.clone()],
            }
        };

        let max = query.max.clamp(1, 200);
        // Filtering happens here, not on the server, so ask for more per request
        // than the caller wants: a 100-row page costs the same as a 25-row one,
        // and most of it is thrown away.
        let filtering = !query.stream_path.is_empty() || query.hide_submitted;
        let page_size = if filtering { 100 } else { max };
        let page_url = |after: u64| {
            let mut url = format!("{base}/api/v9/reviews?max={page_size}");
            for s in &query.states {
                url.push_str(&format!("&state[]={}", esc(s)));
            }
            for u in &who {
                match query.role.as_str() {
                    // "any" falls in with "author": Swarm ANDs distinct filters, so
                    // asking for both at once would return only reviews the user
                    // authored AND reviews on themselves. Authored is the useful
                    // half; the role toggle is there for the other one. Repeated
                    // values of ONE filter are ORed, which is what lets a
                    // half-typed name stand for several people.
                    "reviewer" => url.push_str(&format!("&participants[]={}", esc(u))),
                    _ => url.push_str(&format!("&author[]={}", esc(u))),
                }
            }
            if !query.keywords.is_empty() {
                url.push_str(&format!("&keywords={}", esc(&query.keywords)));
            }
            if after > 0 {
                url.push_str(&format!("&after={}", after));
            }
            url
        };

        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
        {
            Ok(c) => c,
            Err(e) => return Ok(empty(&format!("HTTP client failed: {e}"))),
        };
        // Which changelists belong to this stream (empty set = no stream filter).
        let scope = if query.stream_path.is_empty() {
            None
        } else {
            Some(changes_under(&conn, &query.stream_path))
        };

        // One review row, or None when the stream filter rejects it.
        let parse_row = |r: &serde_json::Value| -> Option<ReviewRow> {
            let state = r.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let author = r.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let versions = r
                .get("versions")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or_default();
            let id = r.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            // Normally the `unwrap_or`: the list projection omits `versions`.
            let change = versions
                .last()
                .and_then(|v| v.get("change"))
                .and_then(|v| v.as_u64())
                .unwrap_or(id);

            // Already submitted: the review is done in practice, whatever Swarm
            // still calls it.
            if query.hide_submitted
                && r.get("commits")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty())
            {
                return None;
            }

            if let Some(scope) = &scope {
                // Any of the review's changelists being in the stream places it:
                // `id` is Swarm's own shadow changelist, `changes` the author's
                // (and each version's), `commits` the submitted ones once it lands.
                let nums = |key: &str| -> Vec<u64> {
                    r.get(key)
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                        .unwrap_or_default()
                };
                let mine = std::iter::once(id)
                    .chain(std::iter::once(change))
                    .chain(nums("changes"))
                    .chain(nums("commits"));
                if !mine.into_iter().any(|c| scope.contains(&c)) {
                    return None;
                }
            }

            let mut reviewers: Vec<String> = r
                .get("participants")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str())
                        .filter(|p| *p != author)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            reviewers.sort();
            reviewers.dedup();
            Some(ReviewRow {
                kind: "review".to_string(),
                id,
                state_label: state_label(&state),
                state,
                author,
                description: r
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                created: r.get("created").and_then(|v| v.as_i64()).unwrap_or(0),
                updated: r.get("updated").and_then(|v| v.as_i64()).unwrap_or(0),
                change,
                pending: r.get("pending").and_then(|v| v.as_bool()).unwrap_or(false),
                commits: r
                    .get("commits")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                    .unwrap_or_default(),
                reviewers,
            })
        };

        // With a stream filter most of a server page can be for other depots —
        // measured ~4-6 kept per 100 with "this stream"+"hide submitted" — so
        // keep pulling until the caller has what it asked for. Bounded by rows
        // SCANNED, not fetches: a fixed 8-page cap under-delivered deep
        // requests, and a filter matching nothing must still not crawl forever.
        const MAX_SCANNED: u32 = 3000;
        let mut scanned: u32 = 0;
        let mut out: Vec<ReviewRow> = Vec::new();
        let mut after = query.after;
        let mut last_seen = 0u64;
        while scanned < MAX_SCANNED {
            let url = page_url(after);
            let resp = match client.get(&url).basic_auth(&conn.user, Some(&ticket)).send() {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    let code = r.status().as_u16();
                    return Ok(empty(&match code {
                        401 | 403 => "Swarm refused the P4 ticket (401) — try logging in again.".into(),
                        _ => format!("Swarm returned HTTP {code}."),
                    }));
                }
                Err(e) => return Ok(empty(&format!("Swarm is unreachable: {e}"))),
            };
            let body: serde_json::Value = match resp.json() {
                Ok(v) => v,
                Err(e) => return Ok(empty(&format!("Swarm sent something unreadable: {e}"))),
            };
            if let Some(err) = body.get("error").and_then(|v| v.as_str()) {
                return Ok(empty(&format!("Swarm: {err}")));
            }
            let raw = body
                .get("reviews")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or_default();
            scanned += raw.len() as u32;
            out.extend(raw.iter().filter_map(&parse_row));
            // `lastSeen` is Swarm's cursor. It repeats on the final page, so a
            // short page is what actually ends the paging.
            let more = raw.len() as u32 == page_size;
            last_seen = if more {
                body.get("lastSeen").and_then(|v| v.as_u64()).unwrap_or(0)
            } else {
                0
            };
            if !more || out.len() as u32 >= max {
                break;
            }
            after = last_seen;
            if after == 0 {
                break;
            }
        }
        Ok(ReviewPage {
            reviews: out,
            last_seen,
            error: String::new(),
        })
    })
    .await
    .map_err(|e| format!("swarm-reviews task failed: {e}"))?
}

/// A review's shelf, written out as a patch the apply pipeline can consume.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPatch {
    /// Temp `.patch` file holding the unified diff.
    pub path: String,
    /// Text files the patch covers.
    pub files: usize,
    /// Files the shelf carries that a patch cannot express (binaries, and adds
    /// or deletes p4 prints no diff for). Named so the dialog can say what it
    /// will not touch instead of quietly dropping them.
    pub skipped: Vec<String>,
}

/// Turn `p4 describe -S -du <change>` into a standard unified diff.
///
/// p4 separates files with `==== //depot/file#rev (text) ====` instead of the
/// `---`/`+++` pair every patch parser looks for, and prefixes the changelist
/// header and the shelved-file list. Rewriting the headers is enough: the hunk
/// bodies are already unified-diff format.
fn shelf_to_patch(describe: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut in_file = false; // inside a text file's hunks
    let mut pending_header: Option<String> = None; // emit only if hunks follow
    for line in describe.lines() {
        if let Some(rest) = line.strip_prefix("==== ") {
            let head = rest.trim_end_matches(" ====").trim();
            // "//depot/file#12 (text)" / "(binary)" / "(text+w)" …
            let (spec, kind) = match head.rsplit_once(" (") {
                Some((s, k)) => (s.trim(), k.trim_end_matches(')')),
                None => (head, ""),
            };
            in_file = false;
            pending_header = None;
            if kind.starts_with("binary") || spec.is_empty() {
                if !spec.is_empty() {
                    skipped.push(strip_rev(spec));
                }
                continue;
            }
            // Both sides name the same depot path, with NO revision: the apply
            // pipeline maps this string with `p4 where`, which refuses a revspec
            // ("A revision specification (# or @) cannot be used here"), so a
            // `#79` here would make every file report as unmapped.
            let bare = strip_rev(spec);
            pending_header = Some(format!("--- {bare}\n+++ {bare}\n"));
            in_file = true;
            continue;
        }
        if !in_file {
            continue; // header, description, "Shelved files ..." list
        }
        if line.starts_with("@@") {
            if let Some(h) = pending_header.take() {
                out.push_str(&h);
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Hunk body. Everything before the first `@@` of a file is noise p4 puts
        // between the header and the first hunk.
        if pending_header.is_none() {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, skipped)
}

/// `//depot/file#12` -> `//depot/file`.
fn strip_rev(spec: &str) -> String {
    match spec.rsplit_once('#') {
        Some((f, r)) if r.chars().all(|c| c.is_ascii_digit()) => f.to_string(),
        _ => spec.to_string(),
    }
}

/// Write `change`'s shelved content out as a patch file and report what it
/// covers. The caller then runs the ordinary patch preview/apply on it, so a
/// review applies through exactly the same path (and the same conflict handling)
/// as a `.patch` from disk.
#[tauri::command]
pub async fn review_patch(conn: P4Conn, change: String) -> Result<ReviewPatch, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if change.trim().is_empty() {
            return Err("This review has no shelved changelist to apply.".into());
        }
        // A review's content is normally a shelf — but a review that was submitted
        // without approval has none (submitting deletes it), and then the
        // changelist itself is the content. Ask what this one is before diffing.
        let shelved = p4::run(&conn, &["describe", "-S", "-s", &change])
            .unwrap_or_default()
            .iter()
            .any(|r| r.get("depotFile").is_some());
        // -S: shelved content, -du: unified diff. P4DIFF is cleared inside
        // run_raw_stdout_diff, so an external diff tool cannot hijack this.
        let args: Vec<&str> = if shelved {
            vec!["describe", "-S", "-du", &change]
        } else {
            vec!["describe", "-du", &change]
        };
        let describe = p4::run_raw_stdout_diff(&conn, &args)?;
        let (patch, _) = shelf_to_patch(&describe);
        let covered: std::collections::HashSet<String> = patch
            .lines()
            .filter_map(|l| l.strip_prefix("--- "))
            .map(|s| s.to_string())
            .collect();
        let files = covered.len();

        // The shelf is the authority on what the review contains: anything it
        // holds that the diff did not cover has to be copied verbatim instead,
        // so it is listed rather than quietly dropped.
        let list_args: Vec<&str> = if shelved {
            vec!["describe", "-S", "-s", &change]
        } else {
            vec!["describe", "-s", &change]
        };
        let shelf: Vec<(String, String)> = p4::run(&conn, &list_args)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| {
                let f = r.get("depotFile").and_then(|v| v.as_str())?;
                let a = r.get("action").and_then(|v| v.as_str()).unwrap_or("");
                Some((f.to_string(), a.to_string()))
            })
            .collect();
        let skipped: Vec<String> = shelf
            .iter()
            .map(|(f, _)| f.clone())
            .filter(|f| !covered.contains(f))
            .collect();

        if shelf.is_empty() {
            return Err(format!("@{change} has no files to apply."));
        }
        // A review on another depot maps nowhere here; say that once, with the
        // depot, instead of reporting every single file as "not found".
        if !shelf.iter().any(|(f, _)| !client_path(&conn, f).is_empty()) {
            let depot = shelf[0]
                .0
                .split('/')
                .take(3)
                .collect::<Vec<_>>()
                .join("/");
            return Err(format!(
                "This review is on {depot}, which this workspace does not map."
            ));
        }
        if files == 0 && skipped.is_empty() {
            return Err("This review's shelf has no changes to apply.".into());
        }
        let path = std::env::temp_dir().join(format!("auger-review-{change}.patch"));
        std::fs::write(&path, patch).map_err(|e| format!("cannot write the patch: {e}"))?;
        Ok(ReviewPatch {
            path: path.display().to_string(),
            files,
            skipped,
        })
    })
    .await
    .map_err(|e| format!("review-patch task failed: {e}"))?
}

/// What happened to one file copied straight from the shelf.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    pub depot: String,
    pub local: String,
    pub action: String,
    /// "copied" | "opened" | "skipped" | "failed"
    pub status: String,
    pub message: String,
}

/// Where a depot path lands in this workspace ("" when it isn't mapped).
fn client_path(conn: &P4Conn, depot: &str) -> String {
    p4::run(conn, &["where", depot])
        .ok()
        .and_then(|recs| {
            recs.first().and_then(|r| {
                r.get("path")
                    .or_else(|| r.get("clientFile"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
        })
        .unwrap_or_default()
}

/// Copy files from a review's shelf verbatim, for the ones a unified diff cannot
/// carry: binaries, and adds that have no base to diff against.
///
/// This is `p4 print` of the shelved revision (`@=change`) straight onto the
/// local path — the same content the review holds, byte for byte. `mode` mirrors
/// the patch dialog: "edit" also opens each file (`p4 edit`, or `p4 add` for a
/// new one) so the result lands in a changelist; "offline" only writes to disk.
///
/// Deletes are deliberately only honoured in "edit" mode, where `p4 delete`
/// records the intent and the file can be reverted. In "offline" mode there
/// would be nothing to undo, so the file is left alone and reported.
#[tauri::command]
pub async fn review_copy_files(
    conn: P4Conn,
    change: String,
    files: Vec<String>,
    mode: String,
) -> Result<Vec<CopyResult>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if change.trim().is_empty() || files.is_empty() {
            return Ok(Vec::new());
        }
        // Each file's action decides how to open it. Where that list comes from
        // depends on the source: a shelf (`-S`) or, for a review submitted without
        // approval, the submitted changelist itself.
        let shelved = p4::run(&conn, &["describe", "-S", "-s", &change])
            .unwrap_or_default()
            .iter()
            .any(|r| r.get("depotFile").is_some());
        let list_args: Vec<&str> = if shelved {
            vec!["describe", "-S", "-s", &change]
        } else {
            vec!["describe", "-s", &change]
        };
        let mut action_of: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Ok(recs) = p4::run(&conn, &list_args) {
            for r in &recs {
                let f = r.get("depotFile").and_then(|v| v.as_str()).unwrap_or("");
                let a = r.get("action").and_then(|v| v.as_str()).unwrap_or("");
                if !f.is_empty() {
                    action_of.insert(f.to_string(), a.to_string());
                }
            }
        }

        let wanted: Vec<String> = files;
        let mut out: Vec<CopyResult> = Vec::new();
        for depot in wanted {
            let action = action_of.get(&depot).cloned().unwrap_or_default();
            let local = client_path(&conn, &depot);
            let mut rep = CopyResult {
                depot: depot.clone(),
                local: local.clone(),
                action: action.clone(),
                status: "failed".into(),
                message: String::new(),
            };
            if local.is_empty() {
                rep.status = "skipped".into();
                rep.message = "not mapped in this workspace".into();
                out.push(rep);
                continue;
            }

            if action == "delete" {
                if mode == "edit" {
                    match p4::run_strict(&conn, &["delete", &depot]) {
                        Ok(_) => {
                            rep.status = "opened".into();
                            rep.message = "opened for delete".into();
                        }
                        Err(e) => rep.message = e,
                    }
                } else {
                    rep.status = "skipped".into();
                    rep.message = "deleted in the review; left alone in offline mode".into();
                }
                out.push(rep);
                continue;
            }

            // Open BEFORE writing: `p4 edit` syncs nothing but clears the
            // read-only flag, and a later write would otherwise be clobbered.
            // An `add` from a SUBMITTED change is already in the depot, so it is
            // opened for edit like any other file — only a shelved add is new here.
            let adding = action == "add" && shelved;
            if mode == "edit" && !adding {
                if let Err(e) = p4::run_strict(&conn, &["edit", &depot]) {
                    rep.message = format!("p4 edit failed: {e}");
                    out.push(rep);
                    continue;
                }
            }
            if let Some(dir) = std::path::Path::new(&local).parent() {
                let _ = std::fs::create_dir_all(dir); // an add may be in a new folder
            }
            if std::path::Path::new(&local).exists() {
                let _ = crate::commands::make_writable(&local);
            }
            // `@=change` is the review's content: the shelved revision, and for a
            // submitted change the revision as of it (checked byte-for-byte
            // against `@change`, and different from `#head`).
            let spec = format!("{depot}@={change}");
            match p4::run_strict(&conn, &["print", "-o", &local, &spec]) {
                Ok(_) => {
                    rep.status = "copied".into();
                }
                Err(e) => {
                    rep.message = format!("p4 print failed: {e}");
                    out.push(rep);
                    continue;
                }
            }
            if mode == "edit" && adding {
                match p4::run_strict(&conn, &["add", &local]) {
                    Ok(_) => {
                        rep.status = "opened".into();
                        rep.message = "opened for add".into();
                    }
                    Err(e) => rep.message = format!("copied, but p4 add failed: {e}"),
                }
            }
            out.push(rep);
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("review-copy task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIBE: &str = "Change 196985 by swarm@swarm-abc on 2026/08/05 08:29:08 *pending*\n\n\tTouchball improvements:\n\t- Less code\n\nShelved files ...\n\n... //d/a.cpp#79 edit\n... //d/b.uasset#3 edit\n\nDifferences ...\n\n==== //d/a.cpp#79 (text) ====\n\n@@ -42,2 +42,2 @@\n ctx\n-old\n+new\n==== //d/b.uasset#3 (binary) ====\n\n==== //d/c.h#1 (text+w) ====\n\n@@ -1,1 +1,2 @@\n one\n+two\n";

    #[test]
    fn rewrites_p4_headers_into_a_unified_diff() {
        let (patch, skipped) = shelf_to_patch(DESCRIBE);
        assert!(patch.starts_with("--- //d/a.cpp\n+++ //d/a.cpp\n@@ -42,2 +42,2 @@\n"));
        assert!(patch.contains("--- //d/c.h\n+++ //d/c.h\n@@ -1,1 +1,2 @@\n"));
        // The changelist header, description and shelved-file list are dropped.
        assert!(!patch.contains("Touchball"));
        assert!(!patch.contains("Shelved files"));
        assert!(!patch.contains("..."));
        assert_eq!(skipped, vec!["//d/b.uasset".to_string()]);
    }

    #[test]
    fn the_result_parses_as_a_patch() {
        let (patch, _) = shelf_to_patch(DESCRIBE);
        let files = crate::commands::parse_patch(&patch);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].depot, "//d/a.cpp");
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[1].depot, "//d/c.h");
        assert_eq!(files[1].hunks.len(), 1);
    }

    #[test]
    fn a_file_whose_section_carries_no_hunk_is_not_announced() {
        // p4 prints a section with no diff for e.g. a type-only change; a `---`
        // with no hunks would make the pipeline report a file it cannot touch.
        let (patch, _) = shelf_to_patch("Differences ...\n\n==== //d/x.cpp#1 (text) ====\n\n");
        assert_eq!(patch, "");
    }

    #[test]
    fn hunk_content_that_looks_like_a_header_survives() {
        let d = "==== //d/a.cpp#1 (text) ====\n\n@@ -1,2 +1,2 @@\n-==== //d/fake.cpp#9 (text) ====\n+ok\n";
        let (patch, skipped) = shelf_to_patch(d);
        // The `====` line inside the hunk is content, but it starts a section as
        // far as this rewriter is concerned. Guard the outcome we depend on: the
        // real file is still announced and no phantom file is skipped.
        assert!(patch.starts_with("--- //d/a.cpp\n"));
        assert!(!skipped.iter().any(|s| s.contains("fake")));
    }

    #[test]
    fn headers_carry_no_revision_because_p4_where_rejects_one() {
        let (patch, _) = shelf_to_patch("==== //d/a.cpp#79 (text) ====

@@ -1,1 +1,1 @@
-a
+b
");
        assert!(!patch.contains('#'), "a revspec in the header makes every file unmappable");
    }

    #[test]
    fn query_values_cannot_break_out_of_the_url() {
        assert_eq!(esc("a b&c=d#e"), "a%20b%26c%3Dd%23e");
        assert_eq!(esc("gaetan.deturche"), "gaetan.deturche");
        assert_eq!(esc("héllo"), "h%C3%A9llo");
    }

    #[test]
    fn state_labels_are_human() {
        assert_eq!(state_label("needsReview"), "Needs Review");
        assert_eq!(state_label("approved:commit"), "Approved (commit)");
        assert_eq!(state_label("weird"), "Weird");
        assert_eq!(state_label(""), "");
    }
}

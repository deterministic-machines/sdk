//! Logging in from a terminal (`dvm-run login`): dvm.run makes an API key
//! for the user who approves the login in their browser, and the terminal
//! saves it where the SDK finds it, so no one copies a key. Over SSH, the
//! user opens the link it prints wherever they like. The Python and JS
//! SDKs' `dvm-run login` is the same.

use std::process::{Command, Stdio};

use dvm_api::{CollectCliLogin, CreatedApiKey, Me, NewCliLogin, StartedCliLogin};
use reqwest::Method;

use crate::client::{Client, api_url_of};
use crate::error::Result;

/// A login's key, which [`save_key`](crate::save_key) keeps where the SDK
/// finds it.
#[derive(Clone, Debug)]
pub struct LoggedIn {
    pub api_url: String,
    /// The user it acts as.
    pub login: String,
    pub key_name: String,
    pub key: String,
}

/// Logs this machine in to dvm.run at `api_url`, else `DVM_API_URL`, else
/// `https://sandbox.dvm.run`, once its user approves the login in their
/// browser: the key it was given. `say` is told what to do; with
/// `open_browser`, the page that approves it opens here.
pub async fn login(
    api_url: Option<&str>,
    open_browser: bool,
    mut say: impl FnMut(&str),
) -> Result<LoggedIn> {
    let url = api_url_of(api_url);
    let terminal = Client::with_key(url.clone(), None);
    let machine = NewCliLogin {
        machine: gethostname::gethostname().to_string_lossy().into_owned(),
    };
    let started: StartedCliLogin = terminal
        .json(Method::POST, "/cli-logins", &[], Some(&machine))
        .await?;
    say(&format!(
        "Approve this login in your browser: {}",
        started.approve_url
    ));
    say(&format!("The page shows the code {}.", started.code));
    if open_browser {
        open_in_browser(&started.approve_url);
    }
    let collect = CollectCliLogin {
        collect_token: started.collect_token,
    };
    let created = loop {
        let answer: Option<CreatedApiKey> = terminal
            .json(Method::POST, "/cli-logins/key", &[], Some(&collect))
            .await?;
        if let Some(created) = answer {
            break created;
        }
    };
    let me: Me = Client::new(url.clone(), created.secret.clone())
        .json(Method::GET, "/me", &[], None::<&()>)
        .await?;
    Ok(LoggedIn {
        api_url: url,
        login: me.login,
        key_name: created.key.name,
        key: created.secret,
    })
}

/// With no browser here, the printed link is the way, so a failure to open
/// one is no error.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        Command::new("cmd").args(["/c", "start", "", url]).spawn()
    } else {
        Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    drop(opener);
}

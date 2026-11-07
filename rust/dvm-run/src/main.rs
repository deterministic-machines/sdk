//! `dvm-run login`: an API key for this machine, approved in the browser,
//! saved where the SDKs find it.

use std::process::ExitCode;

const USAGE: &str = "usage: dvm-run login [--api-url URL] [--no-browser]

Get an API key for this machine, approved in your browser, and save it where
the SDK finds it.

  --api-url URL  the dvm.run to log in to: DVM_API_URL, else https://sandbox.dvm.run
  --no-browser   only print the link to approve the login at";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let (mut api_url, mut open_browser) = (None, true);
    match arguments.next().as_deref() {
        Some("login") => {}
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => return usage_error(),
    }
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--api-url" => match arguments.next() {
                Some(url) => api_url = Some(url),
                None => return usage_error(),
            },
            "--no-browser" => open_browser = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => return usage_error(),
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the login");
    let logged_in = runtime.block_on(dvm_run::login(api_url.as_deref(), open_browser, |line| {
        println!("{line}");
    }));
    let saved = logged_in.and_then(|logged_in| {
        let path = dvm_run::save_key(&logged_in.api_url, &logged_in.key)?;
        Ok((logged_in, path))
    });
    match saved {
        Ok((logged_in, path)) => {
            println!(
                "Logged in to {} as {}, with the key {}.",
                logged_in.api_url, logged_in.login, logged_in.key_name
            );
            println!("It's saved in {}.", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("dvm-run login: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

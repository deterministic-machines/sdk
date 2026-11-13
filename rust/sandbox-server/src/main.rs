mod command_user;
mod commands;
mod files;
mod request_loop;

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    if let Err(error) = request_loop::serve() {
        let answer = dvm_api::SandboxAnswer {
            id: None,
            answer: dvm_api::SandboxAnswerKind::Error(error.to_string()),
        };
        if let Ok(mut line) = serde_json::to_vec(&answer) {
            line.push(b'\n');
            unsafe { libc::write(1, line.as_ptr().cast(), line.len()) };
        }
        std::process::exit(1);
    }
}

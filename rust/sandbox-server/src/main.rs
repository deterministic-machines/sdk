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
        let frame = dvm_api::encode_frame(&answer, &[]);
        unsafe { libc::write(1, frame.as_ptr().cast(), frame.len()) };
        std::process::exit(1);
    }
}

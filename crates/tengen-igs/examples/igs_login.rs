//! Prints what Pandanet says while Tengen signs in, to diagnose sign-in
//! problems. The password is read from the terminal and masked in the output.
//!
//! ```sh
//! cargo run -p tengen-igs --example igs_login -- YOUR_NAME
//! ```

use std::io::{self, BufRead, Write};

use tengen_core::adapter::Credentials;

#[tokio::main]
async fn main() {
    let username = std::env::args().nth(1).expect("usage: igs_login YOUR_NAME");
    let password = match std::env::var("TENGEN_IGS_PASSWORD") {
        Ok(p) => p,
        Err(_) => {
            print!("Pandanet password (shown as you type): ");
            io::stdout().flush().unwrap();
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line).unwrap();
            line.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    let credentials = Credentials { username, password };
    let (result, transcript) =
        tengen_igs::sign_in_transcript(tengen_igs::HOST, tengen_igs::PORT, &credentials).await;
    println!("----- what Pandanet sent -----");
    println!(
        "{}",
        transcript
            .escape_debug()
            .to_string()
            .replace("\\r\\n", "\n")
            .replace("\\n", "\n")
    );
    println!(
        "----- result: {:?} -----",
        result.map_err(|e| e.to_string())
    );
}

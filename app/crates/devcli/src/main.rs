//! Test/verification harness for the CLI library.
//!
//! The shipped app dispatches into `devcli::run_cli`/`devcli::run_internal`
//! before nsh; this binary exists so the acceptance tests and manual checks
//! can drive the same code. Like `main.go`, an optional leading `--dev` selects
//! the internal dispatch and maps every failure to exit 1.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--dev") {
        let code = match devcli::run_internal(&args[1..]) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
        std::process::exit(code);
    }
    let mut input = std::io::stdin();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let code = devcli::run_cli(&args, &mut input, &mut out, &mut err);
    std::process::exit(code);
}

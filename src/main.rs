use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command, flag, root] if command == "daemon" && flag == "--root" => herdr_jest::daemon::serve(&PathBuf::from(root)),
        [command, flag, cwd] if command == "ui" && flag == "--cwd" => herdr_jest::ui::run(&PathBuf::from(cwd)),
        _ => {
            eprintln!("usage: herdr-jest daemon --root <dir> | herdr-jest ui --cwd <dir>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("herdr-jest: {error}");
            ExitCode::FAILURE
        }
    }
}

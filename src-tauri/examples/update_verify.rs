#[path = "../src/update_policy.rs"]
mod update_policy;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = (|| -> Result<(), String> {
        if args.len() != 3 { return Err("usage: update_verify <setup> <signature> <version>".into()); }
        let file = std::fs::File::open(&args[0]).map_err(|e| e.to_string())?;
        let signature = std::fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
        update_policy::verify_reader(file, &signature, &args[2], update_policy::PUBLIC_KEY)
    })();
    match result { Ok(()) => println!("update signature and signed version verified"), Err(error) => {eprintln!("{error}");std::process::exit(1);} }
}

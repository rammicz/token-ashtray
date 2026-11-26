//! Development smoke check. Prints normalized quota data only, never credentials.
use token_ashtray::providers;
fn main() {
    let requested: Vec<_> = std::env::args().skip(1).collect();
    if requested.is_empty() {
        eprintln!("Usage: cargo run --example provider_probe -- <provider-id> ...");
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async{
        let client=providers::client().expect("TLS client");
        for provider in providers::all(){
            let d=provider.descriptor();if !requested.iter().any(|id|id==d.id){continue;}
            match tokio::time::timeout(std::time::Duration::from_secs(20),provider.poll(&client)).await{
                Ok(Ok(card))=>println!("{}",serde_json::json!({"provider":card.id,"status":"live","plan":card.plan,"meters":card.meters})),
                Ok(Err(error))=>println!("{}",serde_json::json!({"provider":d.id,"status":format!("{:?}",error.status),"message":error.message})),
                Err(_)=>println!("{}",serde_json::json!({"provider":d.id,"status":"error","message":"timeout"})),
            }
        }
    });
}

//! Verifies the actual shipped panic strategy, outside Rust's test harness.
fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let survived = runtime.block_on(async {
        let failed = tokio::spawn(async { panic!("simulated provider failure") }).await;
        let healthy = tokio::spawn(async { 42 }).await;
        failed.is_err() && matches!(healthy, Ok(42))
    });
    assert!(survived, "provider panic escaped isolation");
    println!("Provider panic isolated; healthy task completed.");
}

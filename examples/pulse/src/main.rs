use samara::prelude::*;
use pulse::journal;


#[tokio::main]
async fn main() {
    let mut program = Program::builder();
    let journal = journal::install(&mut program);

    println!("Hello, world!");
    
}

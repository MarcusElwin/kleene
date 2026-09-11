//! Print a few generated cases: `cargo run -p callgebra-difftest --example show -- 5`.

use callgebra_difftest::generator::{case_strategy, inserts};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    let mut runner = TestRunner::default();
    for i in 0..n {
        let case = case_strategy().new_tree(&mut runner).unwrap().current();
        println!("-- case {i}");
        for t in &case.tables {
            println!("{};", t.ddl());
            for ins in inserts(t) {
                println!("{ins};");
            }
        }
        println!("{};\n", case.sql);
    }
}

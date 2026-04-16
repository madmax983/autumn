use reqwest::blocking::Client;
use std::thread;
use std::time::Duration;

pub fn run(url: &str, workers: usize, duration: Option<u64>) {
    if let Err(e) = run_inner(url, workers, duration) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

pub fn run_inner(url: &str, workers: usize, duration: Option<u64>) -> Result<(), String> {
    let base_url = url.trim_end_matches('/');
    println!("Simulating traffic to {base_url} with {workers} workers...");

    if workers == 0 {
        return Err("workers must be greater than 0".to_string());
    }

    let mut handles = vec![];

    for _i in 0..workers {
        let base_url = base_url.to_string();
        let handle = thread::spawn(move || {
            let client = Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .unwrap();

            let start = std::time::Instant::now();
            loop {
                if let Some(d) = duration {
                    if start.elapsed().as_secs() >= d {
                        break;
                    }
                } else if start.elapsed().as_secs() >= 1 {
                    break;
                }

                let _ = client.get(&base_url).send();
                thread::sleep(Duration::from_millis(10));
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulate_runs_successfully() {
        let result = run_inner("http://localhost:3000", 1, Some(1));
        assert!(result.is_ok(), "Simulation should run successfully");
    }

    #[test]
    fn test_simulate_zero_workers_error() {
        let result = run_inner("http://localhost:3000", 0, Some(1));
        assert!(result.is_err(), "Simulation should fail with 0 workers");
    }
}

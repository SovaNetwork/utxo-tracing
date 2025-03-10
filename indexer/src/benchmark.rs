use std::time::Duration;

/// Struct to hold the results of a benchmark run
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub blocks_processed: i32,
    pub duration: Duration,
    pub blocks_per_second: f64,
    pub utxos_processed: i32,
    pub utxos_per_second: f64,
    pub memory_usage: u64, // in bytes
}

impl std::fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Processed {} blocks ({} UTXOs) in {:.2} seconds. Speed: {:.2} blocks/sec, {:.2} UTXOs/sec. Memory usage: {:.2} MB",
            self.blocks_processed,
            self.utxos_processed,
            self.duration.as_secs_f64(),
            self.blocks_per_second,
            self.utxos_per_second,
            self.memory_usage as f64 / (1024.0 * 1024.0)
        )
    }
}

// Helper function to get current memory usage
pub fn get_memory_usage() -> u64 {
    #[cfg(target_os = "linux")]
    {
        use std::fs::File;
        use std::io::Read;

        if let Ok(mut file) = File::open("/proc/self/statm") {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Some(value) = contents.split_whitespace().nth(0) {
                    if let Ok(pages) = value.parse::<u64>() {
                        // Convert to bytes (page size is typically 4KB)
                        return pages * 4096;
                    }
                }
            }
        }
    }

    // For macOS/Windows or if Linux method fails
    100 * 1024 * 1024 // Default 100MB estimate
}

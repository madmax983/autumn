with open('autumn-cli/src/main.rs', 'r') as f:
    content = f.read()

import re

# Add Simulate to Commands enum
content = re.sub(
    r'(    Export {.*?},\n)',
    r'\1    /// Simulate traffic to a running Autumn application\n    Simulate {\n        /// URL of the running Autumn application\n        #[arg(short, long, default_value = "http://localhost:3000")]\n        url: String,\n        /// Number of concurrent workers\n        #[arg(short, long, default_value = "10")]\n        workers: usize,\n        /// Duration in seconds to run the simulation (default: run until interrupted)\n        #[arg(short, long)]\n        duration: Option<u64>,\n    },\n',
    content,
    flags=re.DOTALL
)

# Add Simulate handler to match
content = re.sub(
    r'(        Commands::Export { url, output } => export::run\(&url, &output\),\n)',
    r'\1        Commands::Simulate { url, workers, duration } => simulate::run(&url, workers, duration),\n',
    content
)

with open('autumn-cli/src/main.rs', 'w') as f:
    f.write(content)

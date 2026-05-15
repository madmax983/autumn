package com.example.benchmark;

import com.example.benchmark.config.DatabaseUrlFallback;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;

@SpringBootApplication
public class BenchmarkApplication {
    public static void main(String[] args) {
        DatabaseUrlFallback.applyFromEnvironment();
        SpringApplication.run(BenchmarkApplication.class, args);
    }
}

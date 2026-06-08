package com.example.benchmark.config;

import static org.assertj.core.api.Assertions.assertThat;

import java.util.Map;
import java.util.Properties;
import org.junit.jupiter.api.Test;

class DatabaseUrlFallbackTest {
    @Test
    void convertsDatabaseUrlToSpringDatasourceProperties() {
        Properties properties = new Properties();

        DatabaseUrlFallback.apply(
            Map.of("DATABASE_URL", "postgres://benchmark:secret@localhost:5432/benchmark"),
            properties
        );

        assertThat(properties.getProperty("spring.datasource.url"))
            .isEqualTo("jdbc:postgresql://localhost:5432/benchmark");
        assertThat(properties.getProperty("spring.datasource.username")).isEqualTo("benchmark");
        assertThat(properties.getProperty("spring.datasource.password")).isEqualTo("secret");
    }

    @Test
    void keepsSpringSpecificDatasourceWhenConfigured() {
        Properties properties = new Properties();

        DatabaseUrlFallback.apply(
            Map.of(
                "DATABASE_URL", "postgres://benchmark:secret@localhost:5432/benchmark",
                "SPRING_DATASOURCE_URL", "jdbc:postgresql://db:5432/benchmark"
            ),
            properties
        );

        assertThat(properties).doesNotContainKey("spring.datasource.url");
        assertThat(properties).doesNotContainKey("spring.datasource.username");
        assertThat(properties).doesNotContainKey("spring.datasource.password");
    }

    @Test
    void preservesQueryParametersAndDecodesCredentials() {
        DatabaseUrlFallback.DatasourceSettings settings = DatabaseUrlFallback.parse(
            "postgres://bench%20user:p%40ss@db:5432/benchmark?sslmode=disable"
        );

        assertThat(settings.url()).isEqualTo("jdbc:postgresql://db:5432/benchmark?sslmode=disable");
        assertThat(settings.username()).isEqualTo("bench user");
        assertThat(settings.password()).isEqualTo("p@ss");
    }
}

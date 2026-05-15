package com.example.benchmark.config;

import java.net.URI;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.Properties;

/**
 * Applies Heroku-style DATABASE_URL compatibility for standalone benchmark runs.
 */
public final class DatabaseUrlFallback {
    private static final String DATABASE_URL = "DATABASE_URL";
    private static final String SPRING_DATASOURCE_URL = "SPRING_DATASOURCE_URL";
    private static final String SPRING_DATASOURCE_USERNAME = "SPRING_DATASOURCE_USERNAME";
    private static final String SPRING_DATASOURCE_PASSWORD = "SPRING_DATASOURCE_PASSWORD";
    private static final String URL_PROPERTY = "spring.datasource.url";
    private static final String USERNAME_PROPERTY = "spring.datasource.username";
    private static final String PASSWORD_PROPERTY = "spring.datasource.password";

    private DatabaseUrlFallback() {
    }

    /**
     * Sets Spring datasource system properties from DATABASE_URL when Spring-specific
     * datasource settings are not already configured.
     */
    public static void applyFromEnvironment() {
        apply(System.getenv(), System.getProperties());
    }

    static void apply(Map<String, String> environment, Properties properties) {
        if (hasText(properties.getProperty(URL_PROPERTY)) || hasText(environment.get(SPRING_DATASOURCE_URL))) {
            return;
        }

        String databaseUrl = environment.get(DATABASE_URL);
        if (!hasText(databaseUrl)) {
            return;
        }

        DatasourceSettings settings = parse(databaseUrl);
        properties.setProperty(URL_PROPERTY, settings.url());

        if (hasText(settings.username())
            && !hasText(properties.getProperty(USERNAME_PROPERTY))
            && !hasText(environment.get(SPRING_DATASOURCE_USERNAME))) {
            properties.setProperty(USERNAME_PROPERTY, settings.username());
        }

        if (hasText(settings.password())
            && !hasText(properties.getProperty(PASSWORD_PROPERTY))
            && !hasText(environment.get(SPRING_DATASOURCE_PASSWORD))) {
            properties.setProperty(PASSWORD_PROPERTY, settings.password());
        }
    }

    static DatasourceSettings parse(String databaseUrl) {
        String trimmed = databaseUrl.trim();
        if (trimmed.startsWith("jdbc:postgresql:")) {
            return new DatasourceSettings(trimmed, null, null);
        }

        URI uri = URI.create(trimmed);
        String scheme = uri.getScheme();
        if (!"postgres".equals(scheme) && !"postgresql".equals(scheme)) {
            throw new IllegalArgumentException("Unsupported DATABASE_URL scheme: " + scheme);
        }

        String host = uri.getHost();
        if (!hasText(host)) {
            throw new IllegalArgumentException("DATABASE_URL must include a host");
        }

        StringBuilder jdbcUrl = new StringBuilder("jdbc:postgresql://").append(host);
        if (uri.getPort() != -1) {
            jdbcUrl.append(':').append(uri.getPort());
        }

        String path = uri.getRawPath();
        jdbcUrl.append(hasText(path) ? path : "/");

        String query = uri.getRawQuery();
        if (hasText(query)) {
            jdbcUrl.append('?').append(query);
        }

        String username = null;
        String password = null;
        String userInfo = uri.getRawUserInfo();
        if (hasText(userInfo)) {
            String[] parts = userInfo.split(":", 2);
            username = decode(parts[0]);
            if (parts.length > 1) {
                password = decode(parts[1]);
            }
        }

        return new DatasourceSettings(jdbcUrl.toString(), username, password);
    }

    private static String decode(String value) {
        return URLDecoder.decode(value, StandardCharsets.UTF_8);
    }

    private static boolean hasText(String value) {
        return value != null && !value.isBlank();
    }

    record DatasourceSettings(String url, String username, String password) {
    }
}

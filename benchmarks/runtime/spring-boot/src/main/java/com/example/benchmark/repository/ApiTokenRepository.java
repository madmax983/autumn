package com.example.benchmark.repository;

import com.example.benchmark.model.ApiToken;
import org.springframework.data.jpa.repository.JpaRepository;
import java.util.Optional;

public interface ApiTokenRepository extends JpaRepository<ApiToken, Long> {
    Optional<ApiToken> findByToken(String token);
}

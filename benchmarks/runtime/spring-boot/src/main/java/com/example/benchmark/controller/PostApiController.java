package com.example.benchmark.controller;

import com.example.benchmark.model.ApiToken;
import com.example.benchmark.model.Post;
import com.example.benchmark.repository.ApiTokenRepository;
import com.example.benchmark.repository.PostRepository;
import jakarta.validation.Valid;
import org.springframework.data.domain.PageRequest;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.*;
import java.util.List;
import java.util.Map;
import java.util.Optional;

@RestController
@RequestMapping("/api/posts")
public class PostApiController {

    private final PostRepository posts;
    private final ApiTokenRepository tokens;

    public PostApiController(PostRepository posts, ApiTokenRepository tokens) {
        this.posts = posts;
        this.tokens = tokens;
    }

    @GetMapping
    public List<Post> list() {
        return posts.findAllByOrderByCreatedAtDesc(PageRequest.of(0, 50));
    }

    @GetMapping("/{id}")
    public ResponseEntity<Post> show(@PathVariable Long id) {
        return posts.findById(id)
            .map(ResponseEntity::ok)
            .orElse(ResponseEntity.notFound().build());
    }

    @PostMapping
    public ResponseEntity<?> create(@Valid @RequestBody Post body) {
        return ResponseEntity.status(HttpStatus.CREATED).body(posts.save(body));
    }

    @PatchMapping("/{id}")
    public ResponseEntity<Post> update(@PathVariable Long id, @RequestBody Post patch) {
        return posts.findById(id).map(p -> {
            if (patch.getTitle() != null) p.setTitle(patch.getTitle());
            if (patch.getBody() != null)  p.setBody(patch.getBody());
            if (patch.getAuthor() != null) p.setAuthor(patch.getAuthor());
            p.setPublished(patch.isPublished());
            return ResponseEntity.ok(posts.save(p));
        }).orElse(ResponseEntity.notFound().build());
    }

    @DeleteMapping("/{id}")
    public ResponseEntity<Void> delete(@PathVariable Long id) {
        if (!posts.existsById(id)) return ResponseEntity.notFound().build();
        posts.deleteById(id);
        return ResponseEntity.noContent().build();
    }

    @GetMapping("/protected")
    public ResponseEntity<?> protectedStats(
        @RequestHeader(value = "Authorization", required = false) String authHeader
    ) {
        if (authHeader == null || !authHeader.startsWith("Bearer ")) {
            return ResponseEntity.status(HttpStatus.UNAUTHORIZED)
                .body(Map.of("error", "missing or invalid Authorization header"));
        }
        String rawToken = authHeader.substring(7);
        Optional<ApiToken> token = tokens.findByToken(rawToken);
        if (token.isEmpty()) {
            return ResponseEntity.status(HttpStatus.UNAUTHORIZED)
                .body(Map.of("error", "invalid token"));
        }
        return ResponseEntity.ok(Map.of(
            "principal", token.get().getPrincipal(),
            "total_posts", posts.count()
        ));
    }
}

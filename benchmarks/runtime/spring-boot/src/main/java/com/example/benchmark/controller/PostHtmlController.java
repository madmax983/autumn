package com.example.benchmark.controller;

import com.example.benchmark.model.Post;
import com.example.benchmark.repository.PostRepository;
import org.springframework.data.domain.PageRequest;
import org.springframework.stereotype.Controller;
import org.springframework.ui.Model;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;

@Controller
public class PostHtmlController {

    private final PostRepository posts;

    public PostHtmlController(PostRepository posts) { this.posts = posts; }

    @GetMapping("/posts")
    public String list(Model model) {
        model.addAttribute("posts",
            posts.findAllByOrderByCreatedAtDesc(PageRequest.of(0, 50)));
        return "posts/list";
    }

    @GetMapping("/posts/{id}")
    public String show(@PathVariable Long id, Model model) {
        Post post = posts.findById(id).orElseThrow();
        model.addAttribute("post", post);
        return "posts/show";
    }
}

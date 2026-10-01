### Performance

- **feed: `escape` copies clean runs instead of pushing one `char` at a
  time.** Most feed text takes the XML-escape slow path (an apostrophe or
  ampersand is enough), which decoded and pushed every character
  individually. The slow path now walks `char_indices` and flushes each
  maximal clean run with a single `push_str`, emitting an entity or dropping
  the character only at run boundaries. Output is byte-identical;
  measured -11.2% instructions per `Feed::render` on
  `autumn/benches/feed_render.rs` (callgrind), with allocations unchanged.

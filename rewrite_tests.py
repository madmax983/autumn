with open('autumn/src/test.rs', 'r') as f:
    content = f.read()

# Ah! I didn't actually update the `TestApp` implementation of `build()` to pass `merge_routers` and `nest_routers` correctly.
# Oh, `try_build_router_merged` was added, but did I put it into `build()` properly?
# Let's check `build` method.

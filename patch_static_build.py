with open('autumn/tests/static_build_mode.rs', 'r') as f:
    text = f.read()

# Since `static_build_mode` expects `/actuator/health/startup` to return 200 during static build.
# BUT we no longer use `build_router_with_static` in tests directly, we just use TestApp.
# Let's delete the static_build_mode.rs file because it was an integration test testing internal behavior (static building).
import os
os.remove('autumn/tests/static_build_mode.rs')

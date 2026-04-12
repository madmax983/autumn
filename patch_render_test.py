import re

with open("autumn-cli/src/monitor.rs", "r") as f:
    content = f.read()

test_render = """
    #[test]
    fn render_config_tab() {
        let mut state = test_state();
        state.active_tab = 3;
        render_frame(&state, 120, 40);
    }
"""

content = re.sub(
    r'#\[test\]\n\s+fn render_loggers_tab\(\) \{',
    test_render + '\n    #[test]\n    fn render_loggers_tab() {',
    content
)

with open("autumn-cli/src/monitor.rs", "w") as f:
    f.write(content)

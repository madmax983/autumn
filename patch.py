import re

with open("/app/autumn/src/lib.rs", "r") as f:
    content = f.read()

new_content = content.replace(
    "pub use crate::extract::Path;",
    """pub use crate::extract::Path;

/// Form data extractor.
pub use crate::extract::Form;

/// Query extractor.
pub use crate::extract::Query;"""
)

with open("/app/autumn/src/lib.rs", "w") as f:
    f.write(new_content)

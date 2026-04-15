# Oh! Because the module itself is `pub(crate)`, clippy complains if items inside it are `pub(crate)`.
# We should change `pub(crate)` inside those modules to `pub`.
sed -i 's/pub(crate) fn/pub fn/g' autumn/src/router.rs
sed -i 's/pub(crate) struct/pub struct/g' autumn/src/router.rs
sed -i 's/pub(crate) async fn/pub async fn/g' autumn/src/router.rs

sed -i 's/pub(crate) fn/pub fn/g' autumn/src/logging.rs
sed -i 's/pub(crate) struct/pub struct/g' autumn/src/logging.rs
sed -i 's/pub(crate) async fn/pub async fn/g' autumn/src/logging.rs

sed -i 's/FeatureDisabled,/#[allow(dead_code)]\n    FeatureDisabled,/g' autumn/src/telemetry.rs

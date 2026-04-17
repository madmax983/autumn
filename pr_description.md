🤖 Sentinel: [fix chaos channels panic test]

🦠 **Mutants Found:**
The `test_channels_zero_capacity_regression` previously did not send or receive any messages. This allowed bugs where channel operations could panic or fail under a 0-capacity setup to easily go undetected since only channel creation was exercised.

🎯 **Tests Added/Strengthened:**
* Updated `test_channels_zero_capacity_regression` to fully test sending and receiving messages.
* Updated `test_channels_capacity_fuzzing` to assert that message sending successfully works and does not panic on any fuzzed capacity.

⚠️ **Suspected Bugs:**
Operations on 0-capacity (or other unexpected capacities) could panic at runtime because the tests were only validating channel initialization and not the actual send/receive operations.

📊 **Kill Rate:**
High. The tests now verify the entire flow of `Channels` logic on edge capacities rather than just initialization.

🔗 **Havoc Interaction:**
These changes were needed to secure regression tests against edge cases exposed during concurrency/chaos evaluations.

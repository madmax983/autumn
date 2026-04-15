#!/bin/bash
sed -i '/use autumn_harvest::models::NewWorkflowExecution;/d' autumn-harvest/autumn-harvest/tests/signal_tests.rs
sed -i '/use autumn_harvest::schema::harvest_workflow_executions;/d' autumn-harvest/autumn-harvest/tests/signal_tests.rs
sed -i '3i use autumn_harvest::models::NewWorkflowExecution;\nuse autumn_harvest::schema::harvest_workflow_executions;' autumn-harvest/autumn-harvest/tests/signal_tests.rs

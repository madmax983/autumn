DROP TRIGGER IF EXISTS autumn_experiment_change_notify ON autumn_experiment_changes;
DROP FUNCTION IF EXISTS autumn_notify_experiment_change();
DROP TABLE IF EXISTS autumn_experiment_changes;
DROP TABLE IF EXISTS autumn_experiment_overrides;
DROP TABLE IF EXISTS autumn_experiment_assignments;
DROP TABLE IF EXISTS autumn_experiments;
DROP TYPE IF EXISTS autumn_experiment_state;

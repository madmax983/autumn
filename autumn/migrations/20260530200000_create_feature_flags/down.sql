DROP TRIGGER IF EXISTS autumn_flag_change_notify ON feature_flag_changes;
DROP FUNCTION IF EXISTS autumn_notify_flag_change();
DROP TABLE IF EXISTS feature_flag_changes;
DROP TABLE IF EXISTS autumn_feature_flags;

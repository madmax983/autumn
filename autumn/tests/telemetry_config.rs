use autumn_web::config::{AutumnConfig, LogFormat, MockEnv, TelemetryProtocol};
use autumn_web::telemetry::{ResolvedLogFormat, TelemetryRuntime, TraceExport};

fn write_config(contents: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("tempfile");
    std::io::Write::write_all(&mut file, contents.as_bytes()).expect("write config");
    file
}

#[test]
fn telemetry_config_loads_from_toml_and_env() {
    let path = write_config(
        r#"
[telemetry]
enabled = true
service_name = "orders-api"
service_namespace = "acme"
service_version = "1.2.3"
environment = "production"
otlp_endpoint = "http://otel-collector:4317"
protocol = "Grpc"
strict = true
"#,
    );

    let mut config = AutumnConfig::load_from(path.path()).expect("config");
    assert!(config.telemetry.enabled);
    assert_eq!(config.telemetry.service_name, "orders-api");
    assert_eq!(config.telemetry.service_namespace.as_deref(), Some("acme"));
    assert_eq!(config.telemetry.service_version, "1.2.3");
    assert_eq!(config.telemetry.environment, "production");
    assert_eq!(
        config.telemetry.otlp_endpoint.as_deref(),
        Some("http://otel-collector:4317")
    );
    assert_eq!(config.telemetry.protocol, TelemetryProtocol::Grpc);
    assert!(config.telemetry.strict);

    let env = MockEnv::new()
        .with("AUTUMN_TELEMETRY__SERVICE_NAME", "checkout-api")
        .with("AUTUMN_TELEMETRY__PROTOCOL", "HttpProtobuf")
        .with("AUTUMN_TELEMETRY__STRICT", "false");
    config.apply_env_overrides_with_env(&env);

    assert_eq!(config.telemetry.service_name, "checkout-api");
    assert_eq!(config.telemetry.protocol, TelemetryProtocol::HttpProtobuf);
    assert!(!config.telemetry.strict);
}

#[test]
fn telemetry_config_otlp_endpoint_resolves_runtime_metadata() {
    let mut config = AutumnConfig::default();
    config.log.format = LogFormat::Json;
    config.telemetry.enabled = true;
    config.telemetry.service_name = "orders-api".to_owned();
    config.telemetry.service_namespace = Some("acme".to_owned());
    config.telemetry.service_version = "1.2.3".to_owned();
    config.telemetry.environment = "production".to_owned();
    config.telemetry.otlp_endpoint = Some("http://otel-collector:4317".to_owned());
    config.telemetry.protocol = TelemetryProtocol::Grpc;

    let runtime = TelemetryRuntime::from_config(&config.log, &config.telemetry, Some("prod"))
        .expect("runtime");

    assert_eq!(runtime.log_format, ResolvedLogFormat::Json);
    assert!(runtime.warning.is_none());

    match runtime.trace_export {
        TraceExport::Otlp(ref otlp) => {
            assert_eq!(otlp.endpoint, "http://otel-collector:4317");
            assert_eq!(otlp.protocol, TelemetryProtocol::Grpc);
            assert_eq!(otlp.resource.service_name, "orders-api");
            assert_eq!(otlp.resource.service_namespace.as_deref(), Some("acme"));
            assert_eq!(otlp.resource.service_version, "1.2.3");
            assert_eq!(otlp.resource.environment, "production");
        }
        TraceExport::Disabled => panic!("expected otlp trace export"),
    }
}

#[test]
fn telemetry_config_invalid_otlp_falls_back_when_not_strict() {
    let mut config = AutumnConfig::default();
    config.telemetry.enabled = true;
    config.telemetry.service_name = "orders-api".to_owned();
    config.telemetry.otlp_endpoint = Some("otel-collector:4317".to_owned());
    config.telemetry.strict = false;

    let runtime = TelemetryRuntime::from_config(&config.log, &config.telemetry, Some("prod"))
        .expect("runtime");

    assert_eq!(runtime.trace_export, TraceExport::Disabled);
    assert!(
        runtime
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("OTLP")),
        "expected invalid OTLP warning"
    );
}

sed -i 's/loggers: LoggersResponse,/loggers: LoggersResponse,\n    config_props: ConfigPropsResponse,/g' autumn-cli/src/monitor.rs
sed -i 's/loggers: LoggersResponse::default(),/loggers: LoggersResponse::default(),\n            config_props: ConfigPropsResponse::default(),/g' autumn-cli/src/monitor.rs
sed -i 's/self.fetch_loggers(&client);/self.fetch_loggers(&client);\n        self.fetch_config_props(&client);/g' autumn-cli/src/monitor.rs

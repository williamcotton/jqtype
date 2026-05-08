use std::fs;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use jqtype_core::{
    AnalysisMode, AnalyzeOptions, InputShape, JqTypeChecker, OutputFormat, Severity,
};

#[derive(Debug, Parser)]
#[command(name = "jqtype")]
#[command(about = "Infer the output JSON shape of a jq filter")]
struct Cli {
    #[arg(value_name = "FILTER")]
    filter: String,

    #[arg(long, value_name = "PATH", conflicts_with = "sample")]
    input_schema: Option<String>,

    #[arg(long, value_name = "PATH", conflicts_with = "input_schema")]
    sample: Option<String>,

    #[arg(long, value_enum, default_value_t = CliOutput::Compact)]
    output: CliOutput,

    #[arg(long)]
    strict: bool,

    #[arg(long)]
    explain: bool,

    #[arg(long)]
    no_color: bool,

    #[arg(long)]
    debug_ast: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliOutput {
    Compact,
    JsonSchema,
    Tree,
}

impl From<CliOutput> for OutputFormat {
    fn from(value: CliOutput) -> Self {
        match value {
            CliOutput::Compact => OutputFormat::Compact,
            CliOutput::JsonSchema => OutputFormat::JsonSchema,
            CliOutput::Tree => OutputFormat::Tree,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let checker = JqTypeChecker::new();

    if cli.debug_ast {
        match checker.parse_debug_ast(&cli.filter) {
            Ok(ast) => {
                println!("{ast}");
                return Ok(ExitCode::SUCCESS);
            }
            Err(report) => {
                print_diagnostics(&report.diagnostics);
                return Ok(ExitCode::from(2));
            }
        }
    }

    let input = read_input_shape(&cli)?;
    let options = AnalyzeOptions {
        mode: if cli.strict {
            AnalysisMode::Strict
        } else {
            AnalysisMode::Permissive
        },
        output_format: cli.output.into(),
        ..AnalyzeOptions::default()
    };

    let report = checker.analyze_filter(&cli.filter, input, options);
    print_diagnostics(&report.diagnostics);

    match cli.output {
        CliOutput::Compact => println!("{}", report.output.to_compact_string()),
        CliOutput::JsonSchema => println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json_schema_value())?
        ),
        CliOutput::Tree => {
            if let Some(ast) = &report.debug_ast {
                println!("{ast}");
            }
            println!("{}", report.output.to_compact_string());
        }
    }

    if report.has_errors()
        || (cli.strict
            && report
                .diagnostics
                .iter()
                .any(|d| matches!(d.severity, Severity::Warning)))
    {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn read_input_shape(cli: &Cli) -> Result<InputShape, Box<dyn std::error::Error>> {
    if let Some(path) = &cli.input_schema {
        let value = read_json(path)?;
        return Ok(InputShape::from_json_schema(value));
    }

    if let Some(path) = &cli.sample {
        let value = read_json(path)?;
        return Ok(InputShape::from_sample(value));
    }

    Ok(InputShape::Unknown)
}

fn read_json(path: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn print_diagnostics(diagnostics: &[jqtype_core::Diagnostic]) {
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        match &diagnostic.span {
            Some(span) => eprintln!(
                "{severity}: {} at {}..{}",
                diagnostic.message, span.start, span.end
            ),
            None => eprintln!("{severity}: {}", diagnostic.message),
        }
    }
}

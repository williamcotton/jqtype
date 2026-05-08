export type Severity = "Warning" | "Error";

export interface SourceSpan {
  start: number;
  end: number;
}

export interface Diagnostic {
  severity: Severity;
  message: string;
  span: SourceSpan | null;
  source_name: string | null;
}

export const SourceSpan = {
  new(start: number, end: number): SourceSpan {
    return { start, end };
  },
};

export const Diagnostic = {
  warning(message: string, span: SourceSpan | null): Diagnostic {
    return { severity: "Warning", message, span, source_name: null };
  },

  error(message: string, span: SourceSpan | null): Diagnostic {
    return { severity: "Error", message, span, source_name: null };
  },

  withSourceName(diagnostic: Diagnostic, source_name: string | null): Diagnostic {
    return { ...diagnostic, source_name };
  },
};

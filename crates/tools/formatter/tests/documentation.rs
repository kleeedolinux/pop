use pop_formatter::format_documentation_comments;
use pop_foundation::FileId;
use pop_source::SourceFile;

fn format_source(text: &str) -> String {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("source");
    format_documentation_comments(&source)
}

fn inline(tag: &str, attributes: &str, content: &str) -> String {
    format!("--- <{tag}{attributes}>{content}</{tag}>")
}

#[test]
fn expands_inline_contracts_and_separates_sibling_elements() {
    let error_summary = inline("summary", "", "Describes division failures.");
    let case_summary = inline("summary", "", "The divisor is zero.");
    let function_summary = inline("summary", "", "Divides one integer by another.");
    let value_parameter = inline("param", " name=\"value\"", "The dividend.");
    let divisor_parameter = inline("param", " name=\"divisor\"", "The divisor.");
    let returns = inline("returns", "", "The integer quotient.");
    let error = inline(
        "error",
        " type=\"DivideError.Zero\"",
        "The divisor is zero.",
    );
    let source = format!(
        "namespace Example\n\n\
         {error_summary}\n\
         public error DivideError\n\
             {case_summary}\n\
             Zero\n\
         end\n\n\
         {function_summary}\n\
         {value_parameter}\n\
         {divisor_parameter}\n\
         {returns}\n\
         {error}\n\
         public function divide(value: Int, divisor: Int): Result<Int, DivideError>\n\
             return Result.Ok(value / divisor)\n\
         end\n"
    );
    let expected = "namespace Example\n\n\
                    --- <summary>\n\
                    --- Describes division failures.\n\
                    --- </summary>\n\
                    public error DivideError\n\
                        --- <summary>\n\
                        --- The divisor is zero.\n\
                        --- </summary>\n\
                        Zero\n\
                    end\n\n\
                    --- <summary>\n\
                    --- Divides one integer by another.\n\
                    --- </summary>\n\
                    ---\n\
                    --- <param name=\"value\">\n\
                    --- The dividend.\n\
                    --- </param>\n\
                    ---\n\
                    --- <param name=\"divisor\">\n\
                    --- The divisor.\n\
                    --- </param>\n\
                    ---\n\
                    --- <returns>\n\
                    --- The integer quotient.\n\
                    --- </returns>\n\
                    ---\n\
                    --- <error type=\"DivideError.Zero\">\n\
                    --- The divisor is zero.\n\
                    --- </error>\n\
                    public function divide(value: Int, divisor: Int): Result<Int, DivideError>\n\
                        return Result.Ok(value / divisor)\n\
                    end\n";

    assert_eq!(format_source(&source), expected);
}

#[test]
fn preserves_nested_markup_self_closing_elements_and_invalid_xml() {
    let summary = inline("summary", "", "Returns a <see cref=\"Player\"/> value.");
    let ordinary = inline("summary", "", "Ordinary comment.");
    let malformed_attribute = inline("summary", " language=\"pop", "Broken");
    let source = format!(
        "namespace Example\n\n\
         {summary}\n\
         --- <inheritdoc/>\n\
         --- <summary>\n\
         --- Broken\n\
         --- </remarks>\n\
         {malformed_attribute}\n\
         -- {ordinary}\n\
         private function find(): Player\n\
         end\n"
    );
    let expected = format!(
        "namespace Example\n\n\
         --- <summary>\n\
         --- Returns a <see cref=\"Player\"/> value.\n\
         --- </summary>\n\
         ---\n\
         --- <inheritdoc/>\n\
         ---\n\
         --- <summary>\n\
         --- Broken\n\
         --- </remarks>\n\
         {malformed_attribute}\n\
         -- {ordinary}\n\
         private function find(): Player\n\
         end\n"
    );

    assert_eq!(format_source(&source), expected);
}

#[test]
fn formatting_is_idempotent() {
    let summary = inline("summary", "", "Greets the user.");
    let source = format!(
        "namespace Example\n\n\
         {summary}\n\
         private function greet()\n\
         end\n"
    );
    let once = format_source(&source);
    let twice = format_source(&once);

    assert_eq!(twice, once);
}

#[test]
fn preserves_crlf_and_nested_indentation() {
    let summary = inline("summary", "", "A nested declaration.");
    let source =
        format!("namespace Example\r\n    {summary}\r\n    private record Item\r\n    end\r\n");
    let expected = "namespace Example\r\n    --- <summary>\r\n    --- A nested declaration.\r\n    --- </summary>\r\n    private record Item\r\n    end\r\n";

    assert_eq!(format_source(&source), expected);
}

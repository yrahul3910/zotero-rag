use arrow_array::{self, ArrayRef, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema;
use core::fmt;
use std::sync::Arc;
use std::{error::Error, vec::IntoIter};

use super::library::{parse_library, LibraryParsingError};

#[derive(Debug, Clone)]
pub enum ArrowError {
    LibNotFoundError,
    ArrowSchemaError(String),
    PathEncodingError,
    PdfParsingError(String),
}

impl fmt::Display for ArrowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibNotFoundError => write!(f, "Library not found"),
            Self::ArrowSchemaError(msg) => write!(f, "Arrow schema error: {}", msg),
            Self::PathEncodingError => write!(f, "Path contains invalid UTF-8 characters"),
            Self::PdfParsingError(msg) => write!(f, "{}", msg),
        }
    }
}

impl From<LibraryParsingError> for ArrowError {
    fn from(value: LibraryParsingError) -> Self {
        match value {
            LibraryParsingError::LibNotFoundError => Self::LibNotFoundError,
            LibraryParsingError::PdfParsingError(msg) => Self::PdfParsingError(msg),
        }
    }
}

impl From<arrow_schema::ArrowError> for ArrowError {
    fn from(value: arrow_schema::ArrowError) -> Self {
        Self::ArrowSchemaError(value.to_string())
    }
}

impl Error for ArrowError {}

/// Converts Zotero library items to an Arrow RecordBatch.
///
/// This function parses the Zotero library using `parse_library()` and converts
/// the resulting `ZoteroItemMetadata` entries into a structured Arrow RecordBatch.
/// The RecordBatch contains the following columns:
/// - library_key: The unique key for each item in the Zotero library
/// - title: The title of the paper/document
/// - abstract: The abstract of the paper (optional)
/// - notes: Any notes associated with the item (optional)
/// - file_path: Path to the document file
///
/// # Returns
///
/// A `Result` containing either the Arrow `RecordBatch` with all library items
/// or an `ArrowError` if parsing fails or schema conversion fails.
///
/// # Errors
///
/// This function returns an error if:
/// - The Zotero library can't be found or parsed
/// - There's an error creating the Arrow schema
/// - There's an error converting the data to Arrow format
/// - Any file paths contain invalid UTF-8 characters
///
/// # Arguments
///
/// * `start_from` - An optional offset for the SQL query. Useful for debugging, pagination,
///   multi-threading, etc.
/// * `limit` - Optional limit, meant to be used in conjunction with `start_from`.
pub fn library_to_arrow(
    start_from: Option<usize>,
    limit: Option<usize>,
) -> Result<RecordBatchIterator<IntoIter<Result<RecordBatch, arrow_schema::ArrowError>>>, ArrowError>
{
    let lib_items = parse_library(start_from, limit)?;
    log::info!("Finished parsing library items.");

    // Convert ZoteroItemMetadata to something that can be converted to Arrow
    // Need to extract fields and create appropriate Arrow arrays
    let schema = Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("library_key", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("title", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("file_path", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("pdf_text", arrow_schema::DataType::Utf8, false),
    ]));

    // Convert ZoteroItemMetadata to Arrow arrays
    let library_keys = StringArray::from(
        lib_items
            .iter()
            .map(|item| item.metadata.library_key.as_str())
            .collect::<Vec<&str>>(),
    );

    let titles = StringArray::from(
        lib_items
            .iter()
            .map(|item| item.metadata.title.as_str())
            .collect::<Vec<&str>>(),
    );

    let pdf_texts = StringArray::from(
        lib_items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<&str>>(),
    );

    // Convert file paths to strings, returning an error if any path has invalid UTF-8
    let file_paths_vec: Result<Vec<&str>, ArrowError> = lib_items
        .iter()
        .map(|item| {
            item.metadata
                .file_path
                .to_str()
                .ok_or(ArrowError::PathEncodingError)
        })
        .collect();
    let file_paths = StringArray::from(file_paths_vec?);

    let record_batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(library_keys) as ArrayRef,
            Arc::new(titles) as ArrayRef,
            Arc::new(file_paths) as ArrayRef,
            Arc::new(pdf_texts) as ArrayRef,
        ],
    )?;

    let batches = vec![Ok(record_batch)];
    let reader = RecordBatchIterator::new(batches.into_iter(), schema);

    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ftail::Ftail;

    #[test]
    fn test_library_to_arrow_works() {
        Ftail::new().console(log::LevelFilter::Info).init().unwrap();

        let batch_iter = library_to_arrow(Some(0), Some(5));

        assert!(
            batch_iter.is_ok(),
            "Failed to fetch library: {:?}",
            batch_iter.err()
        );

        let mut batch_iter = batch_iter.unwrap();
        // Get the first batch
        let batch = batch_iter
            .next()
            .expect("No batches in iterator")
            .expect("Error in batch");

        assert_eq!(batch.num_columns(), 4, "Expected 4 columns in record batch");
        assert!(batch.num_rows() == 5, "Expected 5 rows in record batch");
    }
}

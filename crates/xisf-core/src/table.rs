//! Table properties: `<Structure>`, `<Table>`, and the rows and cells in one.
//!
//! A table is the one property type whose value cannot be a `<Property>`
//! element. Its shape lives in a `<Structure>` -- an ordered list of typed
//! fields -- and its data in `<Row>`/`<Cell>` elements underneath a `<Table>`.
//!
//! The structure can be shared: a `<Table>` either contains its own
//! `<Structure>` or carries a `<Reference>` to a standalone one defined at the
//! root. That is the same association mechanism used for resolutions and
//! thumbnails, and it exists so a catalogue's column layout is written once
//! however many tables use it.

use crate::err;
use crate::error::Result;
use crate::header::{DataRef, Element, Header};
use crate::property::PropertyType;

/// One column of a table: a name, a type, and how to present it.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    /// The `id` attribute, a property identifier.
    pub id: String,
    pub kind: PropertyType,
    /// A `format` specifier, which affects only how a value is *printed*.
    pub format: Option<String>,
    /// A `header`, for use as a column title.
    pub header: Option<String>,
}

impl Field {
    /// Parse a `<Field>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "Field" {
            return Err(err!(InvalidArgument, "expected <Field>, got <{}>", element.name));
        }
        let id =
            element.attr("id").ok_or_else(|| err!(BadHeader, "a <Field> has no id"))?.to_string();
        let name =
            element.attr("type").ok_or_else(|| err!(BadHeader, "<Field id={id:?}> has no type"))?;
        Ok(Field {
            kind: PropertyType::parse(name)?,
            id,
            format: element.attr("format").map(str::to_owned),
            header: element.attr("header").map(str::to_owned),
        })
    }
}

/// A `<Structure>` element: the ordered fields of a table.
#[derive(Clone, PartialEq, Debug)]
pub struct Structure {
    /// The `uid`, present when the structure stands alone to be referenced.
    pub uid: Option<String>,
    pub fields: Vec<Field>,
}

impl Structure {
    /// Parse a `<Structure>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "Structure" {
            return Err(err!(InvalidArgument, "expected <Structure>, got <{}>", element.name));
        }
        let fields =
            element.children_named("Field").map(Field::parse).collect::<Result<Vec<_>>>()?;
        if fields.is_empty() {
            return Err(err!(BadHeader, "a <Structure> must have at least one <Field>"));
        }
        Ok(Structure { uid: element.attr("uid").map(str::to_owned), fields })
    }
}

/// One cell's value.
///
/// A cell is written exactly as a property value would be, minus the `id`,
/// `type` and `format` attributes it inherits from its field: a scalar in a
/// `value` attribute, a short string as character data, anything larger in a
/// data block that [`crate::Reader`] reads through [`Cell::data`].
#[derive(Clone, PartialEq, Debug)]
pub struct Cell {
    /// The `value` attribute, for scalars and time points.
    pub value: Option<String>,
    /// The block this cell's value lives in, or the character data it holds
    /// directly -- both live on the [`DataRef`], as they do for a property.
    pub data: DataRef,
}

impl Cell {
    /// Parse a `<Cell>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "Cell" {
            return Err(err!(InvalidArgument, "expected <Cell>, got <{}>", element.name));
        }
        Ok(Cell { value: element.attr("value").map(str::to_owned), data: element.data.clone() })
    }

    /// The cell's textual value, if it has one in the header.
    ///
    /// `None` means the value is in a data block, not that the cell is empty.
    pub fn as_str(&self) -> Option<&str> {
        if self.data.location.is_some() {
            return None;
        }
        self.value.as_deref().or(self.data.text.as_deref())
    }
}

/// A `<Table>` element: a table property, with its structure resolved.
#[derive(Clone, PartialEq, Debug)]
pub struct Table {
    /// The `id` attribute: the property this table serialises.
    pub id: String,
    pub structure: Structure,
    pub rows: Vec<Vec<Cell>>,
    pub caption: Option<String>,
    pub comment: Option<String>,
}

impl Table {
    /// Parse a `<Table>` element, resolving its structure against `header`.
    ///
    /// The structure may be a child or a `<Reference>` to a standalone one,
    /// so the whole header is needed to make sense of a single table.
    pub fn parse(element: &Element, header: &Header) -> Result<Self> {
        if element.name != "Table" {
            return Err(err!(InvalidArgument, "expected <Table>, got <{}>", element.name));
        }
        let id =
            element.attr("id").ok_or_else(|| err!(BadHeader, "a <Table> has no id"))?.to_string();

        let structure = match header.associated(element, "Structure").first() {
            Some(found) => Structure::parse(found)?,
            None => {
                return Err(err!(
                    BadHeader,
                    "<Table id={id:?}> has neither a <Structure> nor a reference to one"
                ));
            }
        };

        let mut rows = Vec::new();
        for row in element.children_named("Row") {
            let cells = row.children_named("Cell").map(Cell::parse).collect::<Result<Vec<_>>>()?;
            // The spec requires one cell per field. A row with the wrong
            // number of cells cannot be read column-wise at all: which field
            // a given cell belongs to is decided by position and nothing else.
            if cells.len() != structure.fields.len() {
                return Err(err!(
                    BadHeader,
                    "<Table id={id:?}> row {} has {} cells but the structure declares {} fields",
                    rows.len(),
                    cells.len(),
                    structure.fields.len()
                ));
            }
            rows.push(cells);
        }

        // `rows` and `columns` are optional, but when written they must agree
        // with the data. A disagreement means the file was assembled wrongly.
        let declared = |name: &str| -> Result<Option<usize>> {
            match element.attr(name) {
                None => Ok(None),
                Some(text) => text.trim().parse::<usize>().map(Some).map_err(|_| {
                    err!(BadAttribute, "<Table id={id:?}> {name}={text:?} is not a count")
                }),
            }
        };
        if let Some(count) = declared("rows")?
            && count != rows.len()
        {
            return Err(err!(
                BadHeader,
                "<Table id={id:?}> declares {count} rows but serialises {}",
                rows.len()
            ));
        }
        if let Some(count) = declared("columns")?
            && count != structure.fields.len()
        {
            return Err(err!(
                BadHeader,
                "<Table id={id:?}> declares {count} columns but its structure has {}",
                structure.fields.len()
            ));
        }

        Ok(Table {
            id,
            structure,
            rows,
            caption: element.attr("caption").map(str::to_owned),
            comment: element.attr("comment").map(str::to_owned),
        })
    }

    /// The index of a field by id.
    pub fn column(&self, id: &str) -> Option<usize> {
        self.structure.fields.iter().position(|f| f.id == id)
    }

    /// One row's cell for a named field.
    pub fn cell(&self, row: usize, id: &str) -> Option<&Cell> {
        self.rows.get(row)?.get(self.column(id)?)
    }
}

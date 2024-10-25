use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Search {
    pub value: String,
    pub regex: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Column {
    pub data: String,
    pub name: String,
    pub searchable: bool,
    pub orderable: bool,
    pub search: Search,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Order {
    pub column: usize,
    pub dir: Dir,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Request {
    pub draw: usize,
    pub start: usize,
    pub length: isize,
    pub columns: Vec<Column>,
    pub order: Option<Vec<Order>>,
    pub search: Search,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response<T> {
    pub draw: usize,
    pub records_total: usize,
    pub records_filtered: usize,
    pub data: Vec<T>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

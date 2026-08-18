use std::borrow::Cow;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    Invalid,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Ident(String),
    Index(u64),
    Key(String),
}

/// Grammar: `path := '$' rest | first rest`
pub fn parse_path(path: &str) -> Result<Vec<Segment>, PathError> {
    let mut parser = Parser { s: path, i: 0 };
    if path.is_empty() {
        return Err(PathError::Invalid);
    }
    let segs = if parser.bump_if('$') {
        parser.parse_rest()?
    } else {
        let mut segs = vec![parser.parse_first()?];
        segs.extend(parser.parse_rest()?);
        segs
    };
    if parser.i != parser.s.len() {
        return Err(PathError::Invalid);
    }
    Ok(segs)
}

/// `.length` on array/string allocates a Number, so this cannot return `&Value`.
pub fn resolve_path<'a>(root: &'a Value, path: &str) -> Result<Cow<'a, Value>, PathError> {
    let segs = parse_path(path)?;
    if segs.is_empty() {
        return Ok(Cow::Borrowed(root));
    }

    let mut idx = 0;
    let mut cur = root;
    while idx < segs.len() {
        match step(cur, &segs[idx])? {
            Step::Borrow(next) => {
                cur = next;
                idx += 1;
            }
            Step::Owned(mut owned) => {
                idx += 1;
                while idx < segs.len() {
                    match step(&owned, &segs[idx])? {
                        Step::Borrow(next) => owned = next.clone(),
                        Step::Owned(next) => owned = next,
                    }
                    idx += 1;
                }
                return Ok(Cow::Owned(owned));
            }
        }
    }
    Ok(Cow::Borrowed(cur))
}

enum Step<'a> {
    Borrow(&'a Value),
    Owned(Value),
}

fn step<'a>(cur: &'a Value, seg: &Segment) -> Result<Step<'a>, PathError> {
    match seg {
        Segment::Ident(name) => {
            // Dot `length` is the accessor; `["length"]` is always a field.
            if name == "length" {
                match cur {
                    Value::Array(items) => {
                        return Ok(Step::Owned(Value::from(items.len() as u64)));
                    }
                    Value::String(s) => {
                        return Ok(Step::Owned(Value::from(s.chars().count() as u64)));
                    }
                    Value::Object(obj) => {
                        return obj.get(name).map(Step::Borrow).ok_or(PathError::Missing);
                    }
                    _ => return Err(PathError::Missing),
                }
            }
            match cur {
                Value::Object(obj) => obj.get(name).map(Step::Borrow).ok_or(PathError::Missing),
                _ => Err(PathError::Missing),
            }
        }
        Segment::Index(n) => match cur {
            Value::Array(items) => {
                let i = usize::try_from(*n).ok().filter(|i| *i < items.len());
                i.and_then(|i| items.get(i))
                    .map(Step::Borrow)
                    .ok_or(PathError::Missing)
            }
            Value::Object(obj) => obj
                .get(&n.to_string())
                .map(Step::Borrow)
                .ok_or(PathError::Missing),
            _ => Err(PathError::Missing),
        },
        Segment::Key(key) => match cur {
            Value::Object(obj) => obj.get(key).map(Step::Borrow).ok_or(PathError::Missing),
            _ => Err(PathError::Missing),
        },
    }
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.i..]
    }

    fn peek(&self) -> Option<u8> {
        self.rest().as_bytes().first().copied()
    }

    fn bump(&mut self) {
        if self.i < self.s.len() {
            self.i += 1;
        }
    }

    fn bump_if(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch as u8) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn parse_first(&mut self) -> Result<Segment, PathError> {
        match self.peek() {
            Some(b'[') => self.parse_bracket(),
            Some(b'0'..=b'9') => self.parse_index(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'_') => self.parse_ident(),
            _ => Err(PathError::Invalid),
        }
    }

    fn parse_rest(&mut self) -> Result<Vec<Segment>, PathError> {
        let mut segs = Vec::new();
        loop {
            match self.peek() {
                None => break,
                Some(b'.') => {
                    self.bump();
                    match self.peek() {
                        Some(b'0'..=b'9') => segs.push(self.parse_index()?),
                        Some(b'A'..=b'Z' | b'a'..=b'z' | b'_') => segs.push(self.parse_ident()?),
                        _ => return Err(PathError::Invalid),
                    }
                }
                Some(b'[') => segs.push(self.parse_bracket()?),
                _ => return Err(PathError::Invalid),
            }
        }
        Ok(segs)
    }

    fn parse_ident(&mut self) -> Result<Segment, PathError> {
        let start = self.i;
        match self.peek() {
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'_') => self.bump(),
            _ => return Err(PathError::Invalid),
        }
        while matches!(
            self.peek(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        ) {
            self.bump();
        }
        Ok(Segment::Ident(self.s[start..self.i].to_string()))
    }

    fn parse_index(&mut self) -> Result<Segment, PathError> {
        let start = self.i;
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(PathError::Invalid);
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        let raw = &self.s[start..self.i];
        let n = raw.parse::<u64>().map_err(|_| PathError::Invalid)?;
        Ok(Segment::Index(n))
    }

    fn parse_bracket(&mut self) -> Result<Segment, PathError> {
        if !self.bump_if('[') {
            return Err(PathError::Invalid);
        }
        let seg = match self.peek() {
            Some(b'"' | b'\'') => Segment::Key(self.parse_string()?),
            Some(b'0'..=b'9') => self.parse_index()?,
            _ => return Err(PathError::Invalid),
        };
        if !self.bump_if(']') {
            return Err(PathError::Invalid);
        }
        Ok(seg)
    }

    fn parse_string(&mut self) -> Result<String, PathError> {
        let quote = self.peek().ok_or(PathError::Invalid)?;
        if quote != b'"' && quote != b'\'' {
            return Err(PathError::Invalid);
        }
        self.bump();
        let start = self.i;
        loop {
            match self.s[self.i..].chars().next() {
                None => return Err(PathError::Invalid),
                Some(ch) if ch as u32 == u32::from(quote) => break,
                Some(ch) => self.i += ch.len_utf8(),
            }
        }
        let inner = self.s[start..self.i].to_string();
        self.bump();
        Ok(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grammar_table() {
        assert!(parse_path("$").unwrap().is_empty());
        assert_eq!(
            parse_path("status").unwrap(),
            vec![Segment::Ident("status".into())]
        );
        assert_eq!(
            parse_path("$.status").unwrap(),
            vec![Segment::Ident("status".into())]
        );
        assert_eq!(
            parse_path("$.data.healthy").unwrap(),
            vec![
                Segment::Ident("data".into()),
                Segment::Ident("healthy".into())
            ]
        );
        assert_eq!(
            parse_path("items.0.id").unwrap(),
            vec![
                Segment::Ident("items".into()),
                Segment::Index(0),
                Segment::Ident("id".into())
            ]
        );
        assert_eq!(
            parse_path("items[0].id").unwrap(),
            parse_path("items.0.id").unwrap()
        );
        assert_eq!(
            parse_path("[\"error-code\"]").unwrap(),
            vec![Segment::Key("error-code".into())]
        );
        assert_eq!(
            parse_path("$[\"error-code\"]").unwrap(),
            vec![Segment::Key("error-code".into())]
        );
        assert_eq!(
            parse_path("errors.length").unwrap(),
            vec![
                Segment::Ident("errors".into()),
                Segment::Ident("length".into())
            ]
        );
        assert_eq!(
            parse_path("items[\"0\"].id").unwrap(),
            vec![
                Segment::Ident("items".into()),
                Segment::Key("0".into()),
                Segment::Ident("id".into())
            ]
        );
        assert_eq!(parse_path("error-code"), Err(PathError::Invalid));
    }

    #[test]
    fn reject_bare_hyphenated_and_junk() {
        for path in [
            "error-code",
            "content-type",
            "",
            "$.",
            "$status",
            ".status",
            "items.",
            "items[]",
            "foo.bar-baz",
            "$$",
        ] {
            assert_eq!(parse_path(path), Err(PathError::Invalid), "{path}");
        }
    }

    #[test]
    fn length_accessor_vs_field() {
        let root = json!({
            "errors": ["a", "b"],
            "name": "ab",
            "meta": { "length": 9 },
            "length": 3
        });
        assert_eq!(*resolve_path(&root, "errors.length").unwrap(), json!(2));
        assert_eq!(*resolve_path(&root, "name.length").unwrap(), json!(2));
        assert_eq!(*resolve_path(&root, "meta.length").unwrap(), json!(9));
        assert_eq!(*resolve_path(&root, "meta[\"length\"]").unwrap(), json!(9));
        assert_eq!(*resolve_path(&root, "length").unwrap(), json!(3));
        assert_eq!(
            resolve_path(&root, "errors[\"length\"]").unwrap_err(),
            PathError::Missing
        );
    }

    #[test]
    fn dollar_is_root_and_indexes_match() {
        let root = json!({
            "status": "ok",
            "items": [{ "id": 7 }],
            "error-code": 42
        });
        assert_eq!(resolve_path(&root, "$").unwrap().as_ref(), &root);
        assert_eq!(*resolve_path(&root, "status").unwrap(), json!("ok"));
        assert_eq!(*resolve_path(&root, "$.status").unwrap(), json!("ok"));
        assert_eq!(*resolve_path(&root, "items.0.id").unwrap(), json!(7));
        assert_eq!(*resolve_path(&root, "items[0].id").unwrap(), json!(7));
        assert_eq!(*resolve_path(&root, "[\"error-code\"]").unwrap(), json!(42));
        assert_eq!(
            *resolve_path(&root, "$[\"error-code\"]").unwrap(),
            json!(42)
        );
        assert_eq!(
            resolve_path(&root, "error-code").unwrap_err(),
            PathError::Invalid
        );
        assert_eq!(
            resolve_path(&root, "items.9.id").unwrap_err(),
            PathError::Missing
        );
        let unicode = json!({ "café": 1 });
        assert_eq!(*resolve_path(&unicode, "[\"café\"]").unwrap(), json!(1));
    }
}

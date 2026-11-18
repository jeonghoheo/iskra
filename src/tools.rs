use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const WORKSPACE_DIR: &str = "workspace";

pub fn schemas() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "workspace 폴더 안의 파일을 읽어 내용을 반환합니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "workspace 기준 상대 경로"
                        }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "workspace 폴더 안에 파일을 만들거나 내용 전체를 교체합니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "workspace 기준 상대 경로"
                        },
                        "content": {
                            "type": "string",
                            "description": "저장할 전체 내용"
                        }
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

pub struct Tools {
    root: PathBuf,
}

impl Tools {
    pub fn new() -> io::Result<Self> {
        let root = PathBuf::from(WORKSPACE_DIR);
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(&root)?;
        Ok(Self { root })
    }

    pub fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "read_file" => self.read_file(args),
            "write_file" => self.write_file(args),
            other => Err(format!("알 수 없는 도구입니다: {other}")),
        }
    }

    fn read_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let resolved = self.resolve(path)?;
        if !resolved.is_file() {
            return Err(format!("파일이 아닙니다: {path}"));
        }
        fs::read_to_string(&resolved).map_err(|e| format!("파일 읽기 실패({path}): {e}"))
    }

    fn write_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let content = arg_str(args, "content")?;
        let resolved = self.resolve(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("디렉터리 생성 실패: {e}"))?;
        }
        fs::write(&resolved, content).map_err(|e| format!("파일 쓰기 실패({path}): {e}"))?;
        Ok(format!("{path}에 {} 바이트를 저장했습니다", content.len()))
    }

    fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        if raw.trim().is_empty() {
            return Err("경로가 비어 있습니다".to_string());
        }

        let input = Path::new(raw);
        if input.is_absolute() {
            return Err(format!("절대 경로는 허용되지 않습니다: {raw}"));
        }
        if input.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
            return Err(format!("허용되지 않는 경로 요소가 있습니다: {raw}"));
        }

        let mut current = self.root.clone();
        let mut missing = false;

        for component in input.components() {
            let Component::Normal(part) = component else {
                continue;
            };

            current.push(part);
            if missing {
                continue;
            }

            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    let canon = fs::canonicalize(&current).map_err(|_| {
                        format!("심볼릭 링크를 확인할 수 없어 거부합니다: {raw}")
                    })?;
                    if !canon.starts_with(&self.root) {
                        return Err(format!("workspace 밖으로 향하는 심볼릭 링크는 거부됩니다: {raw}"));
                    }
                    current = canon;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    missing = true;
                }
                Err(e) => return Err(format!("경로 확인 실패({raw}): {e}")),
            }
        }

        if !current.starts_with(&self.root) {
            return Err(format!("workspace 밖 접근은 거부됩니다: {raw}"));
        }
        Ok(current)
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("'{key}' 인자가 문자열로 필요합니다"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iskra-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn allows_paths_inside_workspace() {
        let root = temp_root("inside");
        let tools = Tools { root: root.clone() };

        assert_eq!(tools.resolve("a.txt").unwrap(), root.join("a.txt"));
        assert_eq!(
            tools.resolve("sub/b.txt").unwrap(),
            root.join("sub/b.txt")
        );
        assert_eq!(tools.resolve("./a.txt").unwrap(), root.join("a.txt"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_parent_escape() {
        let root = temp_root("parent");
        let tools = Tools { root: root.clone() };

        assert!(tools.resolve("../secret.txt").is_err());
        assert!(tools.resolve("sub/../../secret.txt").is_err());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_absolute_path() {
        let root = temp_root("absolute");
        let tools = Tools { root: root.clone() };

        assert!(tools.resolve("/etc/passwd").is_err());

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside =
            std::env::temp_dir().join(format!("iskra-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();

        symlink(&outside, root.join("link")).unwrap();
        symlink(outside.join("missing"), root.join("broken")).unwrap();

        let tools = Tools { root: root.clone() };
        assert!(tools.resolve("link/secret.txt").is_err());
        assert!(tools.resolve("broken").is_err());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}

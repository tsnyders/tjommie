import { isAbsolute } from "node:path";

export function localFilePath(path: unknown): path is string {
  return typeof path === "string" && isAbsolute(path) && !/^[\\/]{2}/.test(path);
}

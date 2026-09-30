import { describe, expect, it } from "vitest";

import { baseName, pickedFile, pickedPath } from "./platform";

describe("picked files", () => {
  it("names a file from a browser's input, drop or paste by its own name", () => {
    const file = new File(["x"], "logo.png", { type: "image/png" });
    expect(pickedFile(file)).toEqual({ name: "logo.png", path: null, file });
  });

  it("names a pasted bitmap, which arrives without a name", () => {
    const file = new File(["x"], "", { type: "image/png" });
    const picked = pickedFile(file);
    expect(picked.name).toBe("pasted image");
    expect(picked.file).toBe(file);
  });

  it("names a desktop path by its last part, whichever separator it uses", () => {
    expect(pickedPath("C:\\Users\\me\\logo.png")).toEqual({ name: "logo.png", path: "C:\\Users\\me\\logo.png", file: null });
    expect(pickedPath("/home/me/art/mark.svg").name).toBe("mark.svg");
    expect(baseName("mark.svg")).toBe("mark.svg");
    expect(baseName("C:/mixed\\sep/file.jpg")).toBe("file.jpg");
  });
});

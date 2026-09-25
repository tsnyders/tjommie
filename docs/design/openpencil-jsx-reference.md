# OpenPencil design-jsx reference (from @open-pencil/core 0.15.1)

# OpenPencil design authoring

This reference describes scene creation, not React DOM output. Use the `render` tool for JSX strings, or import `Frame`, `Text`, `renderTree`, and other authoring exports from `@open-pencil/core/design-jsx` in library code. Library exports are not automatically globals in agent `eval`; use only the bindings exposed by that execution environment.

## Composition and layout

- `flex="row"` / `flex="col"` enables auto-layout. Use it for content; reserve explicit `x`/`y` or `position="absolute"` for intentional overlays and artwork. Without layout, children share the origin unless positioned.
- `w` / `h` accept pixels, `"hug"` (content-sized), or `"fill"` (available space in a supported layout parent). Use Hug for notes, cards, and long pages instead of guessing heights. Fixed viewport sizes and artwork geometry are intentional exceptions.
- `gap` controls spacing. `p`, `px`, `py`, and `pt`/`pr`/`pb`/`pl` control padding; longhands override shorthands. There is no margin shorthand.
- `justify="start"|"end"|"center"|"between"` controls the primary axis; `items="start"|"end"|"center"|"stretch"` controls the cross axis. Distribution needs available space: `between` cannot create extra room in a Hug container.
- `grow` distributes available space. Avoid circular Hug/Fill dependencies and redundant fixed widths on growing children. Keep Fill sizing through intermediate containers that should stretch.
- For wrapping text in a column, prefer `w="fill"`; fixed-width text can use `textAutoResize="height"`. `maxLines` / `truncate` are intentional truncation, not fixes for accidental overflow.
- `wrap` and `rowGap` enable wrapped flex rows. `grid`, `columns`, and `rows` enable grid (for example `columns="1fr 200px 1fr"`). Grid children use `colStart`, `rowStart`, `colSpan`, and `rowSpan`. The current grid `gap` shorthand takes precedence over `columnGap` and `rowGap`.
- `flow="auto"|"ltr"|"rtl"` controls container flow; text `dir` controls writing direction. Preserve these separately.
- Use measured node bounds and the existing `arrange` tool for independent artboards. Prefer layout constraints to calculating child coordinates; ordinary JavaScript arithmetic is appropriate when real geometry calculations are needed.

## Paint, text, and artwork

- `bg` / `fill`, `stroke`, and text `color` accept colors and supported variable references. Set colors explicitly for predictable contrast. `fills` accepts structured paints; gradient helpers include `linearGradient`, `radialGradient`, `angularGradient`, and `diamondGradient`.
- `rounded` and `roundedTL`/`roundedTR`/`roundedBL`/`roundedBR` control corners. `strokeWidth`, `opacity`, `rotate`, and `blendMode` control appearance. `overflow="hidden"` clips content; do not hide accidental text overflow to make a broken layout appear correct.
- `effects` accepts structured effects such as `dropShadow`, `innerShadow`, and `layerBlur`. `shadow="offsetX offsetY blur #color"` and `blur` are convenient shorthands.
- Text content belongs inside `Text`. Use `size`, `font`, `weight`, `lineHeight`, `letterSpacing`, `textAlign`, `textDecoration`, and `textCase`. Verify fonts actually load before judging dimensions; do not assume every font is available.
- `Icon` uses an Iconify name, size, and color. Prefer icons to emoji when reliable vector output is needed. Image fills belong on appropriate leaf shapes, not containers whose children must remain visible.
- Design JSX props are the portable authoring interface. Some CSS-style aliases are supported, but this is not a browser CSS engine; do not assume arbitrary HTML, classes, or styles work.

## Variables and components

- Create document variables before referencing them with `designVar('id-or-name')`. `defineVars` groups references; it does not create variable collections.
- COLOR references work in paint props. FLOAT references work in `w`, `h`, `gap`, padding, corner radii, `strokeWidth`, `opacity`, text `size`/`fontSize`, `lineHeight`, and `letterSpacing`. Grid `columnGap`/`rowGap` and wrapped flex `rowGap` also support FLOAT references; grid `gap` overrides both axis-specific gaps. Use numbers or FLOAT references for these scalar props, not CSS unit strings.
- References preserve real graph bindings, not just copied values. Set the intended collection mode on the parent before creating scalar-bound content: initial scalar layout resolves that inherited mode. This does not guarantee automatic scalar layout recomputation after a later mode switch. Verify resulting geometry as well as paint when changing modes. Missing or incorrectly typed scalar variables are errors.
- `bind` maps supported scene-field paths to variable IDs or references when no shorthand exists. Use semantic tokens consistently rather than declaring unused collections.
- A reusable JavaScript function shares source code, not component identity. Use `Component`, `ComponentSet`, and `Instance` for editable main components and linked instances.
- `Instance` resolves an existing component through `of`, `component`, or `componentId`. Component-set children named `variant=Primary`, for example, define variants that can be selected when instantiating the set.
- `Component` and `ComponentSet` accept `properties`, an array of native property definitions (`id`, `name`, `type`, `defaultValue`). `Instance` accepts `properties`, an ID-to-value assignment object. Ordinary nodes do not accept `properties`.
- Child `propertyRefs` connect fields to stable property IDs, for example `[{ propertyId: 'message', field: 'TEXT' }]`. Supported fields are `TEXT`, `VISIBLE`, and `INSTANCE_SWAP`; text and swap references require text and instance nodes respectively. References do not depend on layer names.
- Instance assignments use the native string values (including `'true'` / `'false'` for BOOLEAN properties and component IDs for swaps). For example `Instance({ of: noteId, properties: { message: 'Updated review' } })`. Assignments persist through component synchronization; unknown IDs and invalid values fail rather than silently creating inert overrides. Select variants through component-set variant props, not through instance property assignments.
- Reuse existing local or library components before recreating them. Keep meaningful text, visibility, and swap properties exposed rather than hand-editing cloned child nodes.
- Explicit instance `w` / `h` replace the inherited sizing mode on that axis; omitted dimensions retain the main component's sizing. Authored overrides survive component synchronization. Distinguish those placement constraints from the main component's default size, and verify actual bounds in narrower parents. Do not compensate for a sizing mismatch with guessed heights, clipping, or manually positioned siblings.

## Verification

Inspect structure and actual rendered output. Node counts and `describe` diagnostics do not establish visual fidelity. Check wrapping with longer content, narrower containers, component edits, and relevant modes. Resolve overflow and contrast problems at their source. Reuse IDs returned by creation tools rather than repeatedly searching for the same nodes.

The examples below are executed by the authoring-reference tests. Create the named variables before running a variable-bound example.

## Content-sized review note

```jsx
<Frame name="Review note" w={280} h="hug" flex="col" gap={8} p={16} bg="#FFFFFF">
<Text name="Author" size={12} weight="medium" color="#252A31">June Lee</Text>
<Text name="Message" w="fill" size={12} color="#6B7079">Give the date a little more room at the bottom.</Text>
</Frame>
```

## Variable-bound spacing and typography

```jsx
<Frame name="Bound note" w={280} h="hug" flex="col" gap={designVar('Space/small')} p={designVar('Space/medium')} bg="#FFFFFF">
<Text name="Message" w="fill" size={designVar('Type/body')} lineHeight={designVar('Type/body-leading')} letterSpacing={designVar('Type/body-tracking')} color="#252A31">A note that grows with its content.</Text>
</Frame>
```

## Supported syntax inventory

Generated from the renderer metadata. This inventory lists accepted names, not arbitrary browser CSS support.

**Elements:** `Frame`, `Text`, `Rectangle`, `Ellipse`, `Line`, `Star`, `Polygon`, `Vector`, `Group`, `Section`, `Component`, `ComponentSet`, `Instance`, `View`, `Rect`, `Icon`.

**Helpers:** `solid`, `gradient`, `linearGradient`, `radialGradient`, `angularGradient`, `diamondGradient`, `dropShadow`, `innerShadow`, `layerBlur`, `backgroundBlur`, `foregroundBlur`, `designVar`, `defineVars`.

**Properties:** `name`, `key`, `flex`, `flow`, `dir`, `gap`, `wrap`, `rowGap`, `columnGap`, `justify`, `justifyContent`, `items`, `align`, `alignItems`, `grow`, `w`, `h`, `width`, `height`, `minW`, `maxW`, `minH`, `maxH`, `x`, `y`, `top`, `left`, `position`, `p`, `padding`, `px`, `py`, `pt`, `pr`, `pb`, `pl`, `bg`, `fill`, `fills`, `background`, `backgroundColor`, `stroke`, `border`, `borderColor`, `strokeWidth`, `borderWidth`, `strokeAlign`, `strokeDash`, `rounded`, `borderRadius`, `roundedTL`, `roundedTR`, `roundedBL`, `roundedBR`, `cornerRadius`, `cornerSmoothing`, `opacity`, `blendMode`, `rotate`, `rotation`, `overflow`, `shadow`, `blur`, `effects`, `size`, `fontSize`, `font`, `fontFamily`, `weight`, `fontWeight`, `color`, `text`, `characters`, `content`, `value`, `title`, `textAlign`, `textAlignHorizontal`, `textHorizontalAlignment`, `textAlignVertical`, `textVerticalAlignment`, `textAutoResize`, `lineHeight`, `letterSpacing`, `textDecoration`, `textCase`, `maxLines`, `truncate`, `grid`, `columns`, `rows`, `colStart`, `rowStart`, `col`, `row`, `colSpan`, `rowSpan`, `points`, `pointCount`, `innerRadius`, `label`, `style`, `bind`, `component`, `componentId`, `properties`, `propertyRefs`, `of`.


## Supported properties

[
 {
  "name": "name",
  "description": "OpenPencil name property."
 },
 {
  "name": "key",
  "description": "OpenPencil key property."
 },
 {
  "name": "flex",
  "description": "OpenPencil flex property."
 },
 {
  "name": "flow",
  "description": "OpenPencil flow property."
 },
 {
  "name": "dir",
  "description": "OpenPencil dir property."
 },
 {
  "name": "gap",
  "description": "OpenPencil gap property."
 },
 {
  "name": "wrap",
  "description": "OpenPencil wrap property."
 },
 {
  "name": "rowGap",
  "description": "OpenPencil rowGap property."
 },
 {
  "name": "columnGap",
  "description": "OpenPencil columnGap property."
 },
 {
  "name": "justify",
  "description": "OpenPencil justify property."
 },
 {
  "name": "justifyContent",
  "description": "OpenPencil justifyContent property."
 },
 {
  "name": "items",
  "description": "OpenPencil items property."
 },
 {
  "name": "align",
  "description": "OpenPencil align property."
 },
 {
  "name": "alignItems",
  "description": "OpenPencil alignItems property."
 },
 {
  "name": "grow",
  "description": "OpenPencil grow property."
 },
 {
  "name": "w",
  "description": "OpenPencil w property."
 },
 {
  "name": "h",
  "description": "OpenPencil h property."
 },
 {
  "name": "width",
  "description": "OpenPencil width property."
 },
 {
  "name": "height",
  "description": "OpenPencil height property."
 },
 {
  "name": "minW",
  "description": "OpenPencil minW property."
 },
 {
  "name": "maxW",
  "description": "OpenPencil maxW property."
 },
 {
  "name": "minH",
  "description": "OpenPencil minH property."
 },
 {
  "name": "maxH",
  "description": "OpenPencil maxH property."
 },
 {
  "name": "x",
  "description": "OpenPencil x property."
 },
 {
  "name": "y",
  "description": "OpenPencil y property."
 },
 {
  "name": "top",
  "description": "OpenPencil top property."
 },
 {
  "name": "left",
  "description": "OpenPencil left property."
 },
 {
  "name": "position",
  "description": "OpenPencil position property."
 },
 {
  "name": "p",
  "description": "OpenPencil p property."
 },
 {
  "name": "padding",
  "description": "OpenPencil padding property."
 },
 {
  "name": "px",
  "description": "OpenPencil px property."
 },
 {
  "name": "py",
  "description": "OpenPencil py property."
 },
 {
  "name": "pt",
  "description": "OpenPencil pt property."
 },
 {
  "name": "pr",
  "description": "OpenPencil pr property."
 },
 {
  "name": "pb",
  "description": "OpenPencil pb property."
 },
 {
  "name": "pl",
  "description": "OpenPencil pl property."
 },
 {
  "name": "bg",
  "description": "OpenPencil bg property."
 },
 {
  "name": "fill",
  "description": "OpenPencil fill property."
 },
 {
  "name": "fills",
  "description": "OpenPencil fills property."
 },
 {
  "name": "background",
  "description": "OpenPencil background property."
 },
 {
  "name": "backgroundColor",
  "description": "OpenPencil backgroundColor property."
 },
 {
  "name": "stroke",
  "description": "OpenPencil stroke property."
 },
 {
  "name": "border",
  "description": "OpenPencil border property."
 },
 {
  "name": "borderColor",
  "description": "OpenPencil borderColor property."
 },
 {
  "name": "strokeWidth",
  "description": "OpenPencil strokeWidth property."
 },
 {
  "name": "borderWidth",
  "description": "OpenPencil borderWidth property."
 },
 {
  "name": "strokeAlign",
  "description": "OpenPencil strokeAlign property."
 },
 {
  "name": "strokeDash",
  "description": "OpenPencil strokeDash property."
 },
 {
  "name": "rounded",
  "description": "OpenPencil rounded property."
 },
 {
  "name": "borderRadius",
  "description": "OpenPencil borderRadius property."
 },
 {
  "name": "roundedTL",
  "description": "OpenPencil roundedTL property."
 },
 {
  "name": "roundedTR",
  "description": "OpenPencil roundedTR property."
 },
 {
  "name": "roundedBL",
  "description": "OpenPencil roundedBL property."
 },
 {
  "name": "roundedBR",
  "description": "OpenPencil roundedBR property."
 },
 {
  "name": "cornerRadius",
  "description": "OpenPencil cornerRadius property."
 },
 {
  "name": "cornerSmoothing",
  "description": "OpenPencil cornerSmoothing property."
 },
 {
  "name": "opacity",
  "description": "OpenPencil opacity property."
 },
 {
  "name": "blendMode",
  "description": "OpenPencil blendMode property."
 },
 {
  "name": "rotate",
  "description": "OpenPencil rotate property."
 },
 {
  "name": "rotation",
  "description": "OpenPencil rotation property."
 },
 {
  "name": "overflow",
  "description": "OpenPencil overflow property."
 },
 {
  "name": "shadow",
  "description": "OpenPencil shadow property."
 },
 {
  "name": "blur",
  "description": "OpenPencil blur property."
 },
 {
  "name": "effects",
  "description": "OpenPencil effects property."
 },
 {
  "name": "size",
  "description": "OpenPencil size property."
 },
 {
  "name": "fontSize",
  "description": "OpenPencil fontSize property."
 },
 {
  "name": "font",
  "description": "OpenPencil font property."
 },
 {
  "name": "fontFamily",
  "description": "OpenPencil fontFamily property."
 },
 {
  "name": "weight",
  "description": "OpenPencil weight property."
 },
 {
  "name": "fontWeight",
  "description": "OpenPencil fontWeight property."
 },
 {
  "name": "color",
  "description": "OpenPencil color property."
 },
 {
  "name": "text",
  "description": "OpenPencil text property."
 },
 {
  "name": "characters",
  "description": "OpenPencil characters property."
 },
 {
  "name": "content",
  "description": "OpenPencil content property."
 },
 {
  "name": "value",
  "description": "OpenPencil value property."
 },
 {
  "name": "title",
  "description": "OpenPencil title property."
 },
 {
  "name": "textAlign",
  "description": "OpenPencil textAlign property."
 },
 {
  "name": "textAlignHorizontal",
  "description": "OpenPencil textAlignHorizontal property."
 },
 {
  "name": "textHorizontalAlignment",
  "description": "OpenPencil textHorizontalAlignment property."
 },
 {
  "name": "textAlignVertical",
  "description": "OpenPencil textAlignVertical property."
 },
 {
  "name": "textVerticalAlignment",
  "description": "OpenPencil textVerticalAlignment property."
 },
 {
  "name": "textAutoResize",
  "description": "OpenPencil textAutoResize property."
 },
 {
  "name": "lineHeight",
  "description": "OpenPencil lineHeight property."
 },
 {
  "name": "letterSpacing",
  "description": "OpenPencil letterSpacing property."
 },
 {
  "name": "textDecoration",
  "description": "OpenPencil textDecoration property."
 },
 {
  "name": "textCase",
  "description": "OpenPencil textCase property."
 },
 {
  "name": "maxLines",
  "description": "OpenPencil maxLines property."
 },
 {
  "name": "truncate",
  "description": "OpenPencil truncate property."
 },
 {
  "name": "grid",
  "description": "OpenPencil grid property."
 },
 {
  "name": "columns",
  "description": "OpenPencil columns property."
 },
 {
  "name": "rows",
  "description": "OpenPencil rows property."
 },
 {
  "name": "colStart",
  "description": "OpenPencil colStart property."
 },
 {
  "name": "rowStart",
  "description": "OpenPencil rowStart property."
 },
 {
  "name": "col",
  "description": "OpenPencil col property."
 },
 {
  "name": "row",
  "description": "OpenPencil row property."
 },
 {
  "name": "colSpan",
  "description": "OpenPencil colSpan property."
 },
 {
  "name": "rowSpan",
  "description": "OpenPencil rowSpan property."
 },
 {
  "name": "points",
  "description": "OpenPencil points property."
 },
 {
  "name": "pointCount",
  "description": "OpenPencil pointCount property."
 },
 {
  "name": "innerRadius",
  "description": "OpenPencil innerRadius property."
 },
 {
  "name": "label",
  "description": "OpenPencil label property."
 },
 {
  "name": "style",
  "description": "OpenPencil style property."
 },
 {
  "name": "bind",
  "description": "OpenPencil bind property."
 },
 {
  "name": "component",
  "description": "OpenPencil component property."
 },
 {
  "name": "componentId",
  "description": "OpenPencil componentId property."
 },
 {
  "name": "properties",
  "description": "OpenPencil properties property."
 },
 {
  "name": "propertyRefs",
  "description": "OpenPencil propertyRefs property."
 },
 {
  "name": "of",
  "description": "OpenPencil of property."
 }
]


## Elements

[
 {
  "name": "Frame",
  "runtimeType": "frame",
  "description": "A frame or auto-layout container."
 },
 {
  "name": "Text",
  "runtimeType": "text",
  "description": "A text layer."
 },
 {
  "name": "Rectangle",
  "runtimeType": "rectangle",
  "description": "A rectangle layer."
 },
 {
  "name": "Ellipse",
  "runtimeType": "ellipse",
  "description": "An ellipse layer."
 },
 {
  "name": "Line",
  "runtimeType": "line",
  "description": "A line layer."
 },
 {
  "name": "Star",
  "runtimeType": "star",
  "description": "A star layer."
 },
 {
  "name": "Polygon",
  "runtimeType": "polygon",
  "description": "A polygon layer."
 },
 {
  "name": "Vector",
  "runtimeType": "vector",
  "description": "A vector layer."
 },
 {
  "name": "Group",
  "runtimeType": "group",
  "description": "A group of layers."
 },
 {
  "name": "Section",
  "runtimeType": "section",
  "description": "A canvas section."
 },
 {
  "name": "Component",
  "runtimeType": "component",
  "description": "A reusable component."
 },
 {
  "name": "ComponentSet",
  "runtimeType": "component-set",
  "description": "A component variant set."
 },
 {
  "name": "Instance",
  "runtimeType": "instance",
  "description": "An instance of a component."
 },
 {
  "name": "View",
  "runtimeType": "frame",
  "description": "An alias for Frame."
 },
 {
  "name": "Rect",
  "runtimeType": "rectangle",
  "description": "An alias for Rectangle."
 },
 {
  "name": "Icon",
  "runtimeType": "icon",
  "description": "An Iconify icon."
 }
]


## Helpers

[
 {
  "name": "solid",
  "description": "OpenPencil solid helper."
 },
 {
  "name": "gradient",
  "description": "OpenPencil gradient helper."
 },
 {
  "name": "linearGradient",
  "description": "OpenPencil linearGradient helper."
 },
 {
  "name": "radialGradient",
  "description": "OpenPencil radialGradient helper."
 },
 {
  "name": "angularGradient",
  "description": "OpenPencil angularGradient helper."
 },
 {
  "name": "diamondGradient",
  "description": "OpenPencil diamondGradient helper."
 },
 {
  "name": "dropShadow",
  "description": "OpenPencil dropShadow helper."
 },
 {
  "name": "innerShadow",
  "description": "OpenPencil innerShadow helper."
 },
 {
  "name": "layerBlur",
  "description": "OpenPencil layerBlur helper."
 },
 {
  "name": "backgroundBlur",
  "description": "OpenPencil backgroundBlur helper."
 },
 {
  "name": "foregroundBlur",
  "description": "OpenPencil foregroundBlur helper."
 },
 {
  "name": "designVar",
  "description": "OpenPencil designVar helper."
 },
 {
  "name": "defineVars",
  "description": "OpenPencil defineVars helper."
 }
]


## Authoring examples

### Content-sized review note

```tsx
<Frame name="Review note" w={280} h="hug" flex="col" gap={8} p={16} bg="#FFFFFF">
<Text name="Author" size={12} weight="medium" color="#252A31">June Lee</Text>
<Text name="Message" w="fill" size={12} color="#6B7079">Give the date a little more room at the bottom.</Text>
</Frame>
```

### Variable-bound spacing and typography

```tsx
<Frame name="Bound note" w={280} h="hug" flex="col" gap={designVar('Space/small')} p={designVar('Space/medium')} bg="#FFFFFF">
<Text name="Message" w="fill" size={designVar('Type/body')} lineHeight={designVar('Type/body-leading')} letterSpacing={designVar('Type/body-tracking')} color="#252A31">A note that grows with its content.</Text>
</Frame>
```

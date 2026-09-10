# Bundled map geography

The telemetry map uses **Natural Earth v5.1.2**, a public-domain dataset.
Natural Earth's [terms of use](https://www.naturalearthdata.com/about/terms-of-use/)
allow redistribution and modification without permission or attribution.
The dashboard credits Natural Earth as a courtesy.

The generated `world-map-data.js` contains only geographic outlines and fixed
country label anchors. It contains no telemetry, device locations, or downloaded
scripts. Rendering does not contact an external map provider.

## Pinned sources

- Outlines: [1:110m admin-0 map units GeoJSON](https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_admin_0_map_units.geojson).
  Map units distinguish overseas territories, including French Guiana, from
  their parent country. Units that share a country code highlight together.
- Main country anchors: [1:110m admin-0 countries GeoJSON](https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_admin_0_countries.geojson).
- Additional small-country and territory anchors: [1:10m admin-0 map units DBF](https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/10m_cultural/ne_10m_admin_0_map_units.dbf).

## Transformation

1. Read `ISO_A2_EH` as the country identifier. Keep outlines with an empty
   identifier where no two-letter code exists. The source's country-code
   assignments and boundaries are retained as cartographic data.
2. Read each country's `LABEL_X` longitude and `LABEL_Y` latitude. Prefer the
   1:110m country anchor so small overseas units cannot replace the mainland
   anchor; fill missing codes from the 1:10m map units. DBF text fields are
   UTF-8 and trimmed of trailing NULs and spaces.
3. Simplify each outline ring with Douglas–Peucker, tolerance 0.2 degrees.
   Retain the original ring when simplification leaves fewer than four points
   so tiny islands remain visible. Preserve polygon holes and antimeridian
   segments from the source.
4. Apply an equirectangular projection: `x = 480 + longitude × 2.5`,
   `y = 230 − latitude × 2.5`, rounded to one decimal place. Remove consecutive
   duplicate projected points and close each ring with SVG `Z`.
5. Store anchors as `[code, name, x, y]` and outlines as `[code, svgPath]`.

The result has 183 outline features and 250 label anchors, and fits a
`0 0 960 460` SVG viewBox. Outlines are intentionally simplified at this scale;
small regions still receive a marker even when their outline is not included.
Country counts without an anchor remain visible in the map's unmapped summary.
The full country list remains the textual alternative to the map.

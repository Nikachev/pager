# Offline time-zone lookup

`tz_lookup.js` is an unmodified copy of @photostructure/tz-lookup 11.7.0,
verified against its npm SHA-512 integrity. See `tz_lookup.provenance.json`
for the pinned archive and local SHA-256, and `TZ_LOOKUP_LICENSE` for CC0.
Upstream: https://github.com/photostructure/tz-lookup

Geographic data derives from Evan Siroky's timezone-boundary-builder:
https://github.com/evansiroky/timezone-boundary-builder
Data is licensed under the Open Data Commons Open Database License (ODbL):
https://opendatacommons.org/licenses/odbl/1-0/
The upstream compressed lookup is approximate, particularly near boundaries.
The page labels the result as an estimate. DST and civil-time rules are
provided by the browser's Intl/IANA database, evaluated at board UTC.

The generated HTML embeds the lookup locally and makes no location requests.

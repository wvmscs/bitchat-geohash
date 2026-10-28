-- SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Why a channel is watched: part of the start area, or added by a descent
-- into a cell (D12). Descents are restored at start, unless forgotten.
ALTER TABLE channel ADD COLUMN origin TEXT NOT NULL DEFAULT 'start'
    CHECK (origin IN ('start', 'descent'));

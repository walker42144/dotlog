-- 0002: 默认分类（与 tokens.css 的 8 色分类色板一致）。
-- 用户改名或改色不影响 id，统计按 id 聚合。

INSERT OR IGNORE INTO category (id, name, color, parent_id, sort_order, created_at_ms, updated_at_ms) VALUES
  ('cat-study',   '学习', '#2563eb', '', 1, 0, 0),
  ('cat-work',    '工作', '#0f766e', '', 2, 0, 0),
  ('cat-life',    '生活', '#b45309', '', 3, 0, 0),
  ('cat-sport',   '运动', '#15803d', '', 4, 0, 0),
  ('cat-reading', '阅读', '#7c3aed', '', 5, 0, 0),
  ('cat-fun',     '娱乐', '#be185d', '', 6, 0, 0),
  ('cat-sleep',   '睡眠', '#475569', '', 7, 0, 0),
  ('cat-other',   '其它', '#6b7280', '', 8, 0, 0);

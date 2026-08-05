INSERT INTO folioharbor.roles(role_code,display_name) VALUES
 ('owner','Owner'),('editor','Editor'),('reader','Reader') ON CONFLICT DO NOTHING;
INSERT INTO folioharbor.permissions(permission_code) VALUES
 ('library.manage'),('member.invite'),('holding.view'),('holding.edit'),('item.read'),('item.download') ON CONFLICT DO NOTHING;
INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES
 ('owner','library.manage'),('owner','member.invite'),('owner','holding.view'),('owner','holding.edit'),('owner','item.read'),('owner','item.download'),
 ('editor','holding.view'),('editor','holding.edit'),('editor','item.read'),('editor','item.download'),
 ('reader','holding.view'),('reader','item.read'),('reader','item.download') ON CONFLICT DO NOTHING;

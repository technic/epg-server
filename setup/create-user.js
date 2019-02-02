// Test mongodb environment
db.createUser(
{
    user: "rust",
    pwd: "test",
    roles: [
      { role: "readWrite", db: "epg" }
    ]
});

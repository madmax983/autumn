from django.db import migrations, models


class Migration(migrations.Migration):
    initial = True

    dependencies = []

    operations = [
        migrations.CreateModel(
            name="Post",
            fields=[
                ("id", models.BigAutoField(primary_key=True, serialize=False)),
                ("title", models.TextField()),
                ("body", models.TextField()),
                ("published", models.BooleanField(default=False)),
                ("author", models.TextField()),
                ("created_at", models.DateTimeField(auto_now_add=True)),
                ("updated_at", models.DateTimeField(auto_now=True)),
            ],
            options={"db_table": "posts", "ordering": ["-created_at"]},
        ),
        migrations.CreateModel(
            name="ApiToken",
            fields=[
                ("id", models.BigAutoField(primary_key=True, serialize=False)),
                ("token", models.TextField(unique=True)),
                ("principal", models.TextField()),
                ("created_at", models.DateTimeField(auto_now_add=True)),
            ],
            options={"db_table": "api_tokens"},
        ),
    ]

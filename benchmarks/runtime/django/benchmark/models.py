from django.db import models


class Post(models.Model):
    title = models.TextField()
    body = models.TextField()
    published = models.BooleanField(default=False)
    author = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    class Meta:
        db_table = "posts"
        ordering = ["-created_at"]


class ApiToken(models.Model):
    token = models.TextField(unique=True)
    principal = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        db_table = "api_tokens"

    @classmethod
    def verify(cls, raw_token):
        try:
            return cls.objects.get(token=raw_token).principal
        except cls.DoesNotExist:
            return None

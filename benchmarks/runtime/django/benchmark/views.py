import json
from django.core.serializers.json import DjangoJSONEncoder
from django.http import HttpResponse, JsonResponse
from django.shortcuts import get_object_or_404, render
from django.views import View
from django.views.decorators.csrf import csrf_exempt
from django.utils.decorators import method_decorator

from .models import ApiToken, Post


def _serialize_post(p):
    return {
        "id": p.id,
        "title": p.title,
        "body": p.body,
        "published": p.published,
        "author": p.author,
        "created_at": p.created_at.isoformat(),
        "updated_at": p.updated_at.isoformat(),
    }


def health(request):
    return HttpResponse("ok")


# ── HTML views ────────────────────────────────────────────────────────────────

def posts_list(request):
    posts = Post.objects.order_by("-created_at")[:50]
    return render(request, "posts/list.html", {"posts": posts})


def posts_show(request, pk):
    post = get_object_or_404(Post, pk=pk)
    return render(request, "posts/show.html", {"post": post})


# ── JSON API ──────────────────────────────────────────────────────────────────

@method_decorator(csrf_exempt, name="dispatch")
class PostListCreateView(View):
    def get(self, request):
        posts = Post.objects.order_by("-created_at")[:50]
        return JsonResponse([_serialize_post(p) for p in posts], safe=False,
                            encoder=DjangoJSONEncoder)

    def post(self, request):
        try:
            data = json.loads(request.body)
        except json.JSONDecodeError:
            return JsonResponse({"error": "invalid JSON"}, status=400)

        errors = []
        title = (data.get("title") or "").strip()
        body = (data.get("body") or "").strip()
        author = (data.get("author") or "").strip()

        if not title:
            errors.append("title must not be blank")
        elif len(title) > 255:
            errors.append("title must be 255 characters or fewer")
        if not body:
            errors.append("body must not be blank")
        if not author:
            errors.append("author must not be blank")

        if errors:
            return JsonResponse({"error": "; ".join(errors)}, status=422)

        post = Post.objects.create(
            title=title,
            body=body,
            published=bool(data.get("published", False)),
            author=author,
        )
        return JsonResponse(_serialize_post(post), status=201)


@method_decorator(csrf_exempt, name="dispatch")
class PostDetailView(View):
    def get(self, request, pk):
        post = get_object_or_404(Post, pk=pk)
        return JsonResponse(_serialize_post(post))

    def patch(self, request, pk):
        post = get_object_or_404(Post, pk=pk)
        try:
            data = json.loads(request.body)
        except json.JSONDecodeError:
            return JsonResponse({"error": "invalid JSON"}, status=400)
        for field in ("title", "body", "author", "published"):
            if field in data:
                setattr(post, field, data[field])
        post.save()
        return JsonResponse(_serialize_post(post))

    def delete(self, request, pk):
        post = get_object_or_404(Post, pk=pk)
        post.delete()
        return HttpResponse(status=204)


def api_protected(request):
    auth = request.headers.get("Authorization", "")
    if not auth.startswith("Bearer "):
        return JsonResponse(
            {"error": "missing or invalid Authorization header"}, status=401
        )
    raw = auth[len("Bearer "):]
    principal = ApiToken.verify(raw)
    if principal is None:
        return JsonResponse({"error": "invalid token"}, status=401)
    return JsonResponse({"principal": principal, "total_posts": Post.objects.count()})

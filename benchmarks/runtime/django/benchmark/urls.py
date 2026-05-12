from django.urls import path
from . import views

urlpatterns = [
    path("health", views.health),
    path("posts", views.posts_list),
    path("posts/<int:pk>", views.posts_show),
    path("api/posts", views.PostListCreateView.as_view()),
    path("api/posts/protected", views.api_protected),
    path("api/posts/<int:pk>", views.PostDetailView.as_view()),
]

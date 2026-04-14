# Coming From Other Frameworks

If you think in Spring Boot, Django, or Rails, this guide maps the concepts
you already know to their Autumn equivalents. Same ideas, different syntax.

---

## The 30-Second Version

| You know...         | In Autumn it's...                          |
|---------------------|--------------------------------------------|
| Controller          | A module with `#[get]`/`#[post]` functions |
| Service / Bean      | `#[service]` trait                         |
| Repository / DAO    | `#[repository(Model)]` trait               |
| Model / Entity      | `#[model]` struct                          |
| Dependency injection| Axum extractors (auto-wired from handler params) |
| `application.yml`   | `autumn.toml` + `AUTUMN_*` env vars        |
| Migrations          | Diesel migrations (`diesel migration generate`) |
| Middleware / Filter  | Tower layers + `#[intercept]`              |
| Template engine     | Maud (compile-time HTML macros)            |
| ORM queries         | Diesel query builder                       |

---

## Coming From Spring Boot

### Controllers

**Spring Boot:**

```java
@RestController
@RequestMapping("/api/posts")
public class PostController {
    @Autowired
    private PostService postService;

    @GetMapping
    public List<Post> list() {
        return postService.findAll();
    }

    @GetMapping("/{id}")
    public Post getById(@PathVariable Long id) {
        return postService.findById(id)
            .orElseThrow(() -> new ResponseStatusException(HttpStatus.NOT_FOUND));
    }

    @PostMapping
    @ResponseStatus(HttpStatus.CREATED)
    public Post create(@Valid @RequestBody NewPostDto dto) {
        return postService.create(dto);
    }
}
```

**Autumn:**

```rust
use autumn_web::prelude::*;

// No controller class -- just functions in a module (e.g., src/routes/posts.rs)

#[get("/api/posts")]
async fn list(repo: PgPostRepository) -> AutumnResult<Json<Vec<Post>>> {
    Ok(Json(repo.find_all().await?))
}

#[get("/api/posts/{id}")]
async fn get_by_id(Path(id): Path<i64>, repo: PgPostRepository) -> AutumnResult<Json<Post>> {
    Ok(Json(repo.find_by_id(id).await?))  // 404 if not found
}

#[post("/api/posts")]
async fn create(
    repo: PgPostRepository,
    Valid(Json(dto)): Valid<Json<NewPost>>,
) -> AutumnResult<Json<Post>> {
    Ok(Json(repo.save(&dto).await?))
}
```

Key differences:
- No class, no `@Autowired`. Dependencies are handler parameters -- Autumn
  extracts them automatically.
- Validation via `Valid<Json<T>>` instead of `@Valid @RequestBody`.
- Error handling via `?` and `AutumnResult` instead of exceptions.

### Services and Dependency Injection

**Spring Boot:**

```java
@Service
public class OrderService {
    @Autowired private OrderRepository orderRepo;
    @Autowired private InventoryService inventory;

    public Order placeOrder(OrderRequest req) {
        Order order = orderRepo.save(new Order(req));
        inventory.reserve(order.getId());
        return order;
    }
}
```

**Autumn:**

```rust
#[service]
pub trait OrderService {
    fn deps(order_repo: PgOrderRepository, inventory: InventoryServiceImpl);
    async fn place_order(&self, req: OrderRequest) -> AutumnResult<Order>;
}

impl OrderServiceImpl {
    pub async fn place_order(&self, req: OrderRequest) -> AutumnResult<Order> {
        let order = self.order_repo.save(&req.into()).await?;
        self.inventory.reserve(order.id).await?;
        Ok(order)
    }
}

// In a handler -- just add it as a parameter:
#[post("/orders")]
async fn create_order(svc: OrderServiceImpl, Json(req): Json<OrderRequest>)
    -> AutumnResult<Json<Order>>
{
    Ok(Json(svc.place_order(req).await?))
}
```

Spring's `@Autowired` scans the classpath and creates beans at startup.
Autumn's approach is per-request extraction: each handler parameter is
resolved from the request and app state. No startup scanning, no bean
lifecycle, no circular dependency issues.

### Repositories

**Spring Data JPA:**

```java
@Repository
public interface PostRepository extends JpaRepository<Post, Long> {
    List<Post> findByPublished(boolean published);
    long countByAuthorId(Long authorId);
}
```

**Autumn:**

```rust
#[repository(Post)]
pub trait PostRepository {
    fn find_by_published(published: bool) -> Vec<Post>;
    fn count_by_author_id(author_id: i64) -> i64;
}
```

This is the closest 1:1 mapping in the framework. Autumn parses method names
the same way Spring Data does: `find_by_X_and_Y`, `count_by_X`, `exists_by_X`,
`delete_by_X`. It generates the SQL queries at compile time via Diesel.

### Configuration and Profiles

**Spring Boot:**

```yaml
# application.yml
spring:
  profiles:
    active: dev
  datasource:
    url: jdbc:postgresql://localhost/mydb
server:
  port: 8080
```

**Autumn:**

```toml
# autumn.toml
[server]
port = 8080

[database]
url = "postgres://localhost/mydb"
```

| Spring                                  | Autumn                              |
|-----------------------------------------|-------------------------------------|
| `application.yml`                       | `autumn.toml`                       |
| `application-dev.yml`                   | `autumn-dev.toml`                   |
| `application-prod.yml`                  | `autumn-prod.toml`                  |
| `SPRING_DATASOURCE_URL`                 | `AUTUMN_DATABASE__URL`              |
| `@Value("${server.port}")`              | `config.server.port`                |
| `spring.profiles.active`               | `AUTUMN_PROFILE` or auto-detect     |

Profile smart defaults are built in. Dev gives you pretty logging, permissive
CORS, and fast shutdown. Prod gives you JSON logging, strict CORS, and HSTS.
No `application-dev.yml` required for the common case.

### Security

**Spring Security:**

```java
@PreAuthorize("hasRole('ADMIN')")
@GetMapping("/admin")
public String adminPanel() { return "welcome"; }
```

**Autumn:**

```rust
#[get("/admin")]
#[secured("admin")]
async fn admin_panel() -> &'static str {
    "welcome"
}
```

### Actuator

Both frameworks provide actuator endpoints out of the box:

| Spring Actuator          | Autumn Actuator                |
|--------------------------|--------------------------------|
| `/actuator/health`       | `/actuator/health`             |
| `/actuator/info`         | `/actuator/info`               |
| `/actuator/metrics`      | `/actuator/metrics`            |
| `/actuator/env`          | `/actuator/configprops`        |
| `/actuator/loggers`      | `/actuator/loggers`            |
| `/actuator/scheduledtasks` | `/actuator/scheduledtasks`   |

---

## Coming From Django

### Views / URL routing

**Django:**

```python
# urls.py
urlpatterns = [
    path('posts/', views.list_posts),
    path('posts/<int:pk>/', views.get_post),
]

# views.py
def list_posts(request):
    posts = Post.objects.all()
    return JsonResponse({'posts': list(posts.values())})

def get_post(request, pk):
    post = get_object_or_404(Post, pk=pk)
    return JsonResponse(model_to_dict(post))
```

**Autumn:**

```rust
// Route registration -- similar to urls.py
autumn_web::app()
    .routes(routes![list_posts, get_post])
    .run()
    .await;

// Handlers -- similar to views.py
#[get("/posts")]
async fn list_posts(mut db: Db) -> AutumnResult<Json<Vec<Post>>> {
    let posts = posts::table.load(&mut *db).await?;
    Ok(Json(posts))
}

#[get("/posts/{id}")]
async fn get_post(Path(id): Path<i32>, mut db: Db) -> AutumnResult<Json<Post>> {
    let post = posts::table.find(id).first(&mut *db).await
        .map_err(AutumnError::not_found)?;  // like get_object_or_404
    Ok(Json(post))
}
```

### Models and Migrations

**Django:**

```python
class Post(models.Model):
    title = models.CharField(max_length=200)
    body = models.TextField()
    published = models.BooleanField(default=False)
    created_at = models.DateTimeField(auto_now_add=True)
```

**Autumn:**

```rust
#[model]
pub struct Post {
    #[id]
    pub id: i64,
    pub title: String,
    pub body: String,
    #[default]
    pub published: bool,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}
```

| Django                   | Autumn                                |
|--------------------------|---------------------------------------|
| `python manage.py makemigrations` | `diesel migration generate create_posts` |
| `python manage.py migrate` | `diesel migration run` (or auto at startup) |
| `Model.objects.all()`   | `posts::table.load(&mut *db).await`   |
| `Model.objects.filter()` | `posts::table.filter(...).load()`    |
| `Model.objects.get(pk=1)` | `posts::table.find(1).first()`      |
| `get_object_or_404()`   | `.map_err(AutumnError::not_found)?`   |
| `model.save()`          | `diesel::insert_into(...).values(...)` |
| `ModelSerializer`       | `#[derive(Serialize, Deserialize)]` (via serde) |

Django generates migrations from model changes. In Autumn, you write SQL
migrations by hand (or use `diesel migration generate`). The tradeoff: more
control over SQL, less magic.

### Settings

**Django:**

```python
# settings.py
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'mydb',
    }
}
DEBUG = True
```

**Autumn:**

```toml
# autumn.toml
[database]
url = "postgres://localhost/mydb"

[log]
level = "debug"
```

| Django                          | Autumn                           |
|---------------------------------|----------------------------------|
| `settings.py`                   | `autumn.toml`                    |
| `settings_dev.py`               | `autumn-dev.toml`                |
| `os.environ.get('DB_URL')`      | `AUTUMN_DATABASE__URL`           |
| `DEBUG = True`                  | Auto (`dev` profile in debug builds) |
| Middleware list in `settings.py` | Tower layers, `#[intercept]`    |

### Templates

**Django:**

```html
{% extends "base.html" %}
{% block content %}
  <h1>{{ post.title }}</h1>
  <p>{{ post.body }}</p>
{% endblock %}
```

**Autumn (Maud):**

```rust
fn layout(title: &str, content: Markup) -> Markup {
    html! {
        html { head { title { (title) } } body { (content) } }
    }
}

#[get("/posts/{id}")]
async fn show_post(Path(id): Path<i64>, mut db: Db) -> AutumnResult<Markup> {
    let post = posts::table.find(id).first(&mut *db).await
        .map_err(AutumnError::not_found)?;
    Ok(layout(&post.title, html! {
        h1 { (&post.title) }
        p { (&post.body) }
    }))
}
```

Maud templates are Rust code -- compile-time checked, no template file
mismatches, and full IDE support. The tradeoff: no separate template files
(designers can't edit them independently).

---

## Coming From Rails

### Controllers and Routes

**Rails:**

```ruby
# config/routes.rb
resources :posts

# app/controllers/posts_controller.rb
class PostsController < ApplicationController
  def index
    @posts = Post.all
    render json: @posts
  end

  def show
    @post = Post.find(params[:id])
    render json: @post
  end

  def create
    @post = Post.create!(post_params)
    render json: @post, status: :created
  end

  private
  def post_params
    params.require(:post).permit(:title, :body)
  end
end
```

**Autumn:**

```rust
// src/routes/posts.rs

#[get("/posts")]
async fn index(repo: PgPostRepository) -> AutumnResult<Json<Vec<Post>>> {
    Ok(Json(repo.find_all().await?))
}

#[get("/posts/{id}")]
async fn show(Path(id): Path<i64>, repo: PgPostRepository) -> AutumnResult<Json<Post>> {
    Ok(Json(repo.find_by_id(id).await?))
}

#[post("/posts")]
async fn create(
    repo: PgPostRepository,
    Valid(Json(params)): Valid<Json<NewPost>>,
) -> AutumnResult<Json<Post>> {
    Ok(Json(repo.save(&params).await?))
}

// In main.rs
autumn_web::app()
    .routes(routes![posts::index, posts::show, posts::create])
    .run()
    .await;
```

No `resources :posts` shorthand yet. You declare each route explicitly. The
`#[repository]` macro gives you the CRUD methods, but you wire routes
manually.

### Active Record vs. Diesel

| Rails (Active Record)        | Autumn (Diesel)                          |
|------------------------------|------------------------------------------|
| `Post.all`                   | `posts::table.load(&mut *db).await`      |
| `Post.find(1)`              | `posts::table.find(1).first(&mut *db).await` |
| `Post.where(published: true)` | `posts::table.filter(posts::published.eq(true))` |
| `Post.create!(attrs)`       | `diesel::insert_into(posts::table).values(&new_post)` |
| `post.update!(title: "new")` | `diesel::update(posts::table.find(id)).set(...)` |
| `post.destroy`              | `diesel::delete(posts::table.find(id))` |
| `Post.count`                | `posts::table.count().get_result(&mut *db)` |
| Callbacks (`before_save`)   | Mutation hooks (`#[repository(Post, hooks = MyHooks)]`) |

Or use the `#[repository]` macro for a higher-level API:

```rust
repo.find_all().await           // Post.all
repo.find_by_id(1).await        // Post.find(1)
repo.save(&new_post).await      // Post.create!(attrs)
repo.update(1, &changes).await  // post.update!(attrs)
repo.delete_by_id(1).await      // post.destroy
repo.count().await               // Post.count
```

### Migrations

**Rails:**

```bash
rails generate migration CreatePosts title:string body:text
rails db:migrate
```

**Autumn:**

```bash
diesel migration generate create_posts
# Edit up.sql and down.sql by hand
diesel migration run
```

Rails generates migration content from the command line. Diesel generates empty
`up.sql`/`down.sql` files that you fill in with SQL. More verbose, but you
have full control over the SQL.

### Before/After Filters

**Rails:**

```ruby
class ApplicationController < ActionController::Base
  before_action :authenticate_user!
end

class AdminController < ApplicationController
  before_action :require_admin
end
```

**Autumn:**

```rust
// Per-handler authentication
#[get("/admin")]
#[secured("admin")]
async fn admin_panel() -> &'static str { "welcome" }

// Per-group middleware
autumn_web::app()
    .scoped("/admin", AuthLayer::new(), routes![admin_panel, admin_settings])
    .run()
    .await;
```

### Background Jobs

**Rails (Sidekiq/ActiveJob):**

```ruby
class CleanupJob < ApplicationJob
  def perform
    Post.where('created_at < ?', 30.days.ago).destroy_all
  end
end

# Scheduled via sidekiq-cron
CleanupJob.perform_later
```

**Autumn:**

```rust
#[scheduled(every = "24h", name = "cleanup")]
async fn cleanup(state: AppState) -> AutumnResult<()> {
    let mut db = state.db().await?;
    diesel::delete(posts::table.filter(
        posts::created_at.lt(chrono::Utc::now().naive_utc() - chrono::Duration::days(30))
    )).execute(&mut *db).await?;
    Ok(())
}

// Register in main:
autumn_web::app()
    .tasks(tasks![cleanup])
    .run()
    .await;
```

No Redis or external job queue needed for simple scheduled tasks. For durable
workflows with retries and complex DAGs, see `autumn-harvest`.

### Convention vs. Configuration

| Convention            | Rails                      | Autumn                        |
|-----------------------|----------------------------|-------------------------------|
| Table naming          | `Post` → `posts`           | `Post` → `posts` (same)      |
| Insert struct naming  | N/A (same model)           | `Post` → `NewPost`           |
| Update struct naming  | N/A (same model)           | `Post` → `UpdatePost`        |
| Repo struct naming    | N/A (Active Record)        | `Post` → `PgPostRepository`  |
| Service struct naming | N/A                        | `OrderService` → `OrderServiceImpl` |
| Config file           | `config/database.yml`      | `autumn.toml`                 |
| Profile config        | `config/environments/`     | `autumn-{profile}.toml`       |

---

## Concept Translation Cheat Sheet

| Concept                | Spring Boot            | Django                | Rails                  | Autumn                          |
|------------------------|------------------------|-----------------------|------------------------|---------------------------------|
| Entry point            | `@SpringBootApplication` | `manage.py`         | `config/application.rb` | `#[autumn_web::main]`          |
| Route definition       | `@GetMapping`          | `urlpatterns`         | `routes.rb`            | `#[get("/path")]`              |
| Request handler        | Controller method      | View function         | Controller action      | `async fn` with extractors      |
| DI container           | Spring IoC             | N/A (manual)          | N/A (manual)           | Axum extractors                 |
| ORM                    | JPA/Hibernate          | Django ORM            | Active Record          | Diesel                          |
| Data model             | `@Entity`              | `models.Model`        | `ActiveRecord::Base`   | `#[model]`                      |
| Repository             | `JpaRepository`        | Manager               | Active Record          | `#[repository(Model)]`          |
| Service layer          | `@Service`             | Service class         | Service object         | `#[service]`                    |
| Validation             | `@Valid`               | `Form.is_valid()`     | `validates`            | `Valid<T>` + `validator`        |
| Error handling         | `@ExceptionHandler`    | Middleware             | `rescue_from`          | `AutumnResult` + `?`            |
| Auth annotation        | `@PreAuthorize`        | `@login_required`     | `before_action`        | `#[secured("role")]`            |
| Config file            | `application.yml`      | `settings.py`         | `config/*.yml`         | `autumn.toml`                   |
| Profiles               | `spring.profiles`      | `DJANGO_SETTINGS`     | `RAILS_ENV`            | `AUTUMN_PROFILE`                |
| Background tasks       | `@Scheduled`           | Celery                | Sidekiq                | `#[scheduled(every = "5m")]`    |
| Template engine        | Thymeleaf              | Django templates      | ERB                    | Maud (compile-time HTML)        |
| Middleware             | Servlet Filter         | Middleware             | Rack middleware         | Tower layers                    |
| Health check           | Actuator               | Custom                | Custom                 | Built-in `/health`              |
| Migrations             | Flyway/Liquibase       | `manage.py migrate`   | `rails db:migrate`     | `diesel migration run`          |
| CLI                    | Spring CLI             | `manage.py`           | `rails`                | `autumn`                        |
| Hot reload             | Spring DevTools        | Auto-reload           | `rails s`              | `autumn dev`                    |

---

## The Mindset Shift

### No runtime reflection

Spring, Django, and Rails all use runtime introspection to discover
controllers, models, and services. Autumn resolves everything at compile time.
If it compiles, the wiring is correct.

### Errors are values, not exceptions

There is no `try/catch`. Errors flow through `Result<T, E>` and the `?`
operator. This means every error path is visible in the type signature.

### No global state

Spring has an application context. Django has `settings`. Rails has
`Rails.application`. Autumn passes state explicitly through extractors. If a
handler needs the database, it declares `db: Db` in its parameters.

### Compile-time guarantees

- Type-safe SQL queries (Diesel catches column mismatches at compile time)
- Type-safe HTML templates (Maud is Rust code, not string interpolation)
- Type-safe route parameters (a `Path<i32>` that receives "abc" fails at
  the extractor, not in your handler)

The compiler catches more, so the runtime surprises you less.

/* Native ABI regressions: build as a shared module, no external state. */
#include <emacs-module.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

int plugin_is_GPL_compatible;

static emacs_value fail(emacs_env *env, const char *message)
{
  emacs_value data = env->make_string(env, message, (ptrdiff_t)strlen(message));
  data = env->funcall(env, env->intern(env, "list"), 1, &data);
  env->non_local_exit_signal(env, env->intern(env, "error"), data);
  return NULL;
}

static emacs_value copy_bytes(emacs_env *env, ptrdiff_t nargs,
                              emacs_value *args, void *data)
{
  (void)nargs;
  (void)data;
  ptrdiff_t required = -7;
  if (!env->copy_string_contents(env, args[0], NULL, &required))
    return NULL;
  if (required <= 0 || (uintmax_t)required > SIZE_MAX - 2)
    return fail(env, "invalid module sizing result");
  unsigned char *buf = malloc((size_t)required + 2);
  if (!buf)
    return fail(env, "fixture allocation failed");
  memset(buf, 0x55, (size_t)required + 2);
  ptrdiff_t short_size = required - 1;
  bool copied = env->copy_string_contents(env, args[0], (char *)buf, &short_size);
  emacs_value symbol, exit_data;
  enum emacs_funcall_exit code = env->non_local_exit_get(env, &symbol, &exit_data);
  env->non_local_exit_clear(env);
  bool unchanged = true;
  for (ptrdiff_t i = 0; i < required + 2; ++i)
    unchanged = unchanged && buf[i] == 0x55;
  emacs_value expected_data[2] = {env->make_integer(env, required - 1),
                                 env->make_integer(env, required)};
  emacs_value expected = env->funcall(env, env->intern(env, "list"), 2, expected_data);
  emacs_value equality_args[2] = {expected, exit_data};
  emacs_value equal = env->funcall(env, env->intern(env, "equal"), 2, equality_args);
  if (copied || code != emacs_funcall_exit_signal || short_size != required ||
      !unchanged || !env->eq(env, symbol, env->intern(env, "memory-buffer-too-small")) ||
      !env->is_not_nil(env, equal)) {
    free(buf);
    return fail(env, "short-buffer contract violated");
  }
  ptrdiff_t capacity = required + 2;
  copied = env->copy_string_contents(env, args[0], (char *)buf, &capacity);
  if (!copied) {
    free(buf);
    return NULL;
  }
  if (capacity != required || buf[required - 1] != 0 ||
      buf[required] != 0x55 || buf[required + 1] != 0x55) {
    free(buf);
    return fail(env, "copy length/NUL/canary contract violated");
  }
  ptrdiff_t exact_size = required;
  copied = env->copy_string_contents(env, args[0], (char *)buf, &exact_size);
  if (!copied || exact_size != required || buf[required - 1] != 0 ||
      buf[required] != 0x55 || buf[required + 1] != 0x55) {
    free(buf);
    return fail(env, "exact-fit contract violated");
  }
  emacs_value result = env->make_unibyte_string(env, (char *)buf, required - 1);
  free(buf);
  return result;
}

/* Calling the second thunk while an exit is pending must do nothing. After
   clearing it we call that thunk once and GC with the exit's handles alive. */
static emacs_value capture(emacs_env *env, ptrdiff_t nargs,
                           emacs_value *args, void *data)
{
  (void)nargs;
  (void)data;
  emacs_value result = env->funcall(env, args[0], 0, NULL);
  emacs_value symbol, exit_data;
  enum emacs_funcall_exit code = env->non_local_exit_get(env, &symbol, &exit_data);
  if (code == emacs_funcall_exit_return)
    return result;
  (void)env->funcall(env, args[1], 0, NULL);
  emacs_value symbol_after, data_after;
  enum emacs_funcall_exit after = env->non_local_exit_get(env, &symbol_after, &data_after);
  env->non_local_exit_clear(env);
  if (after != code || !env->eq(env, symbol, symbol_after) ||
      !env->eq(env, exit_data, data_after))
    return fail(env, "pending exit was overwritten");
  result = env->funcall(env, args[1], 0, NULL);
  if (env->non_local_exit_check(env) != emacs_funcall_exit_return)
    return NULL;
  env->funcall(env, env->intern(env, "garbage-collect"), 0, NULL);
  emacs_value out[4] = {env->make_integer(env, code), symbol, exit_data, result};
  return env->funcall(env, env->intern(env, "list"), 4, out);
}

static emacs_value propagate(emacs_env *env, ptrdiff_t nargs,
                             emacs_value *args, void *data)
{
  (void)nargs;
  (void)data;
  emacs_value result = env->funcall(env, args[0], 0, NULL);
  emacs_value symbol, exit_data;
  enum emacs_funcall_exit code = env->non_local_exit_get(env, &symbol, &exit_data);
  if (code == emacs_funcall_exit_return)
    return result;
  env->non_local_exit_clear(env);
  if (code == emacs_funcall_exit_throw)
    env->non_local_exit_throw(env, symbol, exit_data);
  else
    env->non_local_exit_signal(env, symbol, exit_data);
  return NULL;
}

static void bind(emacs_env *env, const char *name, ptrdiff_t arity,
                 emacs_value (*function)(emacs_env *, ptrdiff_t, emacs_value *, void *))
{
  emacs_value args[2] = {env->intern(env, name),
                        env->make_function(env, arity, arity, function,
                                           "Native module interoperability regression.", NULL)};
  env->funcall(env, env->intern(env, "defalias"), 2, args);
}

int emacs_module_init(struct emacs_runtime *runtime)
{
  emacs_env *env = runtime->get_environment(runtime);
  bind(env, "native-interop-copy", 1, copy_bytes);
  bind(env, "native-interop-capture", 2, capture);
  bind(env, "native-interop-propagate", 1, propagate);
  return env->non_local_exit_check(env) != emacs_funcall_exit_return;
}
